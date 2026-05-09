# acs 実装概要

## 概要

`acs` は ARES Project 向けのコマンド群をまとめた Rust 製 CLI です。現在の主なコマンドは `control`、`io`、`monitor`、`route`、`send` で、DS4 入力の取得、シリアル監視、入力から出力へのルーティング、ダミーパケット送信を行います。

全体としては、`app/cli` が実行条件を解決し、`serial` と `session` が実行基盤を提供し、共有の入力フレーム型を介して `pipeline` が入力を加工し、`output` が送信フォーマットへ変換する構成です。

## 全体アーキテクチャ

1. `src/main.rs` が `app::cli::run()` を呼び、サブコマンドへ振り分けます。
2. `app/cli` が引数、設定ファイル、既定値、ポート名、ログ保存先を解決します。
   port ごとの baud / display は CLI の `PORT@BAUD,DISPLAY` や config object から個別解決されます。
   monitor の display では `line` / `packet` / `wrap` / `crlf` も指定でき、表示の改行単位や CR/LF の描画を切り替えられます。
3. `session` が監視対象ポート、出力ポート、ダッシュボード、ログファイルを初期化します。
4. 入力があるコマンドでは `serial` の monitor thread がデータを受け、`SessionEvent` としてメインループへ渡します。
5. メインループは共有の `IngressFrame` を組み立て、`control` と `route` では `pipeline` がそのフレームをフィルタ・変換・分類・配送し、送信先ごとの `DispatchPlan` を返します。
6. `send` と `control` では `output` が必要なフォーマットへエンコードし、`SerialWriter` がシリアルへ書き込みます。
7. `ui` がターミナルダッシュボードを差分描画し、`logger` がログへ入出力を保存します。

## コマンド別の流れ

### `control`

- `input/ds4_hid` が DUALSHOCK 4 を列挙・選択して HID レポートを読みます。
- `pipeline` で `Ds4ToCompact -> OutputEncode(指定 FORMAT)` の変換を行います。
- 結果をメイン出力ポートへ送信し、同時に出力ポート自身や追加 monitor ポートも監視できます。
- `PacketACv6` エンコーダは enable 状態、シーケンス番号、操作プロファイルを内部状態として保持します。
- `FORMAT=PacketMv1` の場合は、同じ `PacketACv6` エンコーダ出力を `PacketMv1` へ削減して送信します。

### `monitor`

- 1 個以上のシリアル入力を監視し、受信内容をダッシュボードとログへ記録します。
- display 設定の `packet` 指定では受信チャンク単位、`line` 指定では改行単位、`crlf` 指定では ASCII/UTF-8 内の CR/LF を実改行として表示します。

### `io`

- `send` の実行基盤を使い、複数の送信ポートと受信ポートを同一ダッシュボードで扱います。
- `-i` が受信、`-o` が送信で、各ポートの baud / format / display / rate は `PORT@BAUD,DISPLAY,FORMAT,RATE` のようにポート文字列へ直接付けます。
- 値なしの `-i` / `-o` では、利用可能なシリアルポート、フォーマット、ボーレート、送信レートを対話式に選択できます。

### `route`

- CLI または設定ファイルで入力 ID、出力 ID、パイプライン定義を解決します。
- 指定が薄い場合は `merge` や `one-to-one` のテンプレート、またはデフォルトの broadcast パイプラインで補完します。
- 各入力フレームは `PipelineEngine` に渡され、配送先出力ごとに複製されて送信されます。

### `send`

- 出力形式に応じたダミーペイロードを生成します。
- 20ms 周期で連番ループのダミーペイロードを繰り返し送信し、送信履歴を monitor と同じ UI で表示します。
- 複数の出力ポートを同時に扱え、出力ごとに baud / format / display を分けて設定できます。
- 出力ポートの受信側や、追加で指定した monitor ポートも同時に監視できます。
- 現時点で `packetacv6`、`packetacv6usb`、`packetjfv1`、`packetufv1`、`roverupgeneral`、`roverdowngeneral` のダミーデータ送信に対応しています。

## 主要モジュール

### `app/cli`

- サブコマンドの分岐、ヘルプ表示、バージョン表示
- CLI 引数と設定ファイルの統合
- 設定ディレクトリとログディレクトリの自動解決
- `route` のテンプレート解決とパイプライン構築

### `input`

- HID API を使った DS4 デバイスの列挙と選択
- DS4 生レポートの正規化
- compact 8 バイト形式への変換

### `pipeline`

- `filter -> transform -> classify -> route` から成る小さなデータフローエンジン
- 現在は `AllowAll` filter、`Identity` / `Ds4ToCompact` / `OutputEncode` transform、`Broadcast` / `SourceMap` router を提供
- 処理コアは `session` ではなく共有の入力フレーム型に依存し、実行基盤との結合を抑えています

### `output`

- 出力形式名と実装の対応付け
- `PacketACv6` の状態付きエンコード
- `PacketJFv1` / `PacketUFv1` のダミーフレーム生成
- CRC16-CCITT-FALSE 計算

### `serial` / `session` / `ui`

- シリアルポートの自動選択と入出力ラッパ
- monitor thread とメインループの接続
- テキストダッシュボード、ログ保存、エラー状態表示
- Space キーによる表示停止と Ctrl-C による終了

## 設定とデータの扱い

- 各コマンドは CLI 引数から直接必要な値を組み立てます。
- `route` は built-in template を使ってパイプラインを構成します。
- ログ出力先は既定でリポジトリ内の `./logs` です。

## ビルド・配布まわり

- `build.rs` が Git の branch / commit / dirty 状態 / source kind をビルド時に埋め込みます。
- `acs --version` や `acs version` で、そのビルドがどのソースから作られたか確認できます。
- `Makefile` と `scripts/` が初期化、ビルド、リリース、グローバル導入、更新を担当します。
- ルートの `acs` スクリプトは `cargo run --release -- ...` を呼ぶローカルランチャーです。

## 補足

- `packetacv6` のプロファイル切替時には `afplay` を使った効果音再生を試みますが、環境や音声ファイルが無い場合はそのまま処理を続行します。
- 仕様メモやフォーマット仕様は `docs/` 以下に既存ドキュメントとして分離されています。
