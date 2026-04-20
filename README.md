# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

主なコマンドは次のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- `acs route`: シリアル入力を設定に応じてシリアル出力へ振り分ける
- `acs send`: 指定形式のダミーデータを継続してシリアルポートへ送信する
- `acs xbee-test`: `base` / `rover` の 2 ポート間で PacketACv6 / PacketJFv1 の往復試験を行う。`flood` / `ping-pong` を切り替えられ、ヘッダに実際の表示更新 fps も表示する

## グローバルインストール

通常はリポジトリのルートで、まず次を実行してください。

```bash
make init
```

そのあとでグローバルインストールを行います。

```bash
make install
```

- 既定では、このチェックアウトの「コミット済みの HEAD」の内容だけを使ってグローバルインストールします。未コミット変更は含まれません。
- 通常は `~/.cargo/bin/acs` に入るので、そのディレクトリに `PATH` が通っていればどのディレクトリからでも `acs` を実行できます。
- 初回インストール時には、標準ユーザ設定ディレクトリへ `config.example/` の内容を「未作成のものだけ」コピーします。
- すでにグローバルに `acs` が入っている場合、`make install` は再インストールせず終了します。差し替えたい場合は `make update` か `make sync-code` を使ってください。

特定の ref からインストールしたい場合は `TAG` / `BRANCH` / `COMMIT` を 1 つだけ指定できます。

```bash
make install TAG=v0.1.0
make install BRANCH=main
make install COMMIT=50d3137a75b821d46b5308f7c7693e513836e11d
```

インストールされたビルド情報は次で確認できます。

```bash
acs --version
acs version
```

## 更新方法

### `make sync-code`

ローカルで編集した内容を、そのままグローバルの `acs` に反映したいときに使います。

```bash
make sync-code
```

- 現在の working tree から再インストールするので、未コミット変更も取り込みます。
- 既存のグローバル設定ファイルはそのまま残ります。
- dirty な状態から反映した場合は、`acs --version` に `dirty` が出るので見分けられます。

### `make update`

すでに入っているグローバル版を、配布済みの ref に更新したいときに使います。

```bash
make update
make update TAG=v0.1.0
make update BRANCH=main
make update COMMIT=50d3137a75b821d46b5308f7c7693e513836e11d
```

- 引数なしなら GitHub 上の最新 tag を使って更新します。
- `TAG` / `BRANCH` / `COMMIT` を指定すると、その ref を使って更新します。
- `make install` と同様に、設定例のコピーは「まだ無いものだけ」です。

## 基本的な使い方

まずは全体のヘルプを確認してください。

```bash
acs --help
acs help control
acs help monitor
acs help route
acs help send
acs help xbee-test
```

よく使う基本コマンドは次のとおりです。

```bash
acs ports
acs controllers
acs control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
acs monitor --port /dev/ttyUSB0
acs route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
acs send --port /dev/ttyUSB0 --format PacketJFv1
acs send -o main=/dev/ttyUSB0@115200,hex,packetjfv1 -o sub=/dev/ttyUSB1@921600,utf8+packet,packetacv6
acs xbee-test --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --ac-rate 100 --jf-rate 100
acs xbee-test --mode ping-pong --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --ac-rate 100
acs --version
```

- 1 台だけコントローラーやシリアルポートが見つかる場合は、自動選択されます。
- `--config` には JSON ファイルだけでなくディレクトリも指定できます。
- 詳しいオプションや表示形式は `acs help <command>` を参照してください。
- `acs xbee-test` は表示更新が遅い場合も受信レートとエラー率の集計を優先し、packet 表示は別キューで追いかけます。

## 設定ファイル

`--config` を省略した場合は、次の順で設定を探します。

1. カレントディレクトリの `config/`
2. カレントディレクトリの `acs.config.json`
3. 標準ユーザ設定ディレクトリ

分割設定の例は [config.example](config.example)、単一ファイルの例は [acs.config.example.json](acs.config.example.json) を参照してください。

標準ディレクトリは次のとおりです。

- macOS の設定ディレクトリ: `~/Library/Application Support/acs`
- macOS のログディレクトリ: `~/Library/Logs/acs`
- Linux の設定ディレクトリ: `$XDG_CONFIG_HOME/acs` または `~/.config/acs`
- Linux のログディレクトリ: `$XDG_STATE_HOME/acs/logs` または `~/.local/state/acs/logs`

## その他の `make` コマンド

詳細は `make help` を見るのが早いです。よく使うものだけ挙げると次のとおりです。

- `make init`: Rust が未導入の環境を初期化し、ローカル release ビルドまで実行する
- `make build` / `make build-release`: ローカルでビルドする
- `make fmt` / `make test`: 整形とテストを実行する
- `make sync-config` / `make unsync-config`: ローカル設定をグローバル設定オーバーレイへ同期・解除する
- `make uninstall` / `make purge`: グローバルの `acs` を削除する
- `make paths`: 標準の設定・ログ・バイナリ配置先を表示する
- `make release VERSION=...`: バージョン更新、テスト、ビルド、コミット、タグ作成をまとめて行う
