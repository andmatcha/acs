# `app/cli` モジュール

## 責務

- `acs` のコマンドラインインターフェース全体を担当します。
- 引数解析、既定値解決、実行モードごとの組み立てを行います。

## 主な処理

- `mod.rs` で `control` / `monitor` / `route` / `send` / `xbee-mock` / `xbee-rtt` / `xbee-test` / `controllers` / `ports` / `version` を振り分けます。
- `control.rs` で DS4 入力を `compact` 化し、出力フォーマットへエンコードして送信します。
- `monitor.rs` で 1 個以上のシリアル入力を監視します。
- `route.rs` で入力・出力・パイプライン・テンプレートを解決して中継処理を起動します。
- `send.rs` でダミーペイロードを一定周期で送信します。
- `xbee_rtt.rs` で対称な XBee RTT プロトコルを実装し、通常の 1-port/2-PC 実行と、明示的な 2-port/1-PC ローカルペア実行の両方を扱います。必要なら `--show-wire` で hex dump、`--show-protocol` で意味付き frame log も出せます。
- `xbee_test.rs` で `base` / `remote` 間の AU/RU と AD/RD の往復試験と、その片側だけを動かす `xbee-mock` を実装します。
- `paths.rs` と `signal.rs` でログの保存先解決と Ctrl-C 停止を共通化しています。

## 実装の要点

- 引数解析は外部 CLI クレートに依存せず、各コマンドが手書きで行っています。
- CLI 引数から直接ポートやログディレクトリを最終解決してから `session` と `pipeline` を起動します。
- `route` は組み込みテンプレートを扱えます。
