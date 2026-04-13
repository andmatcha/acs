# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

現在の第一段階で実装されている機能は以下のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- `acs route`: 1 つ以上のシリアル入力を、設定に応じて 1 つ以上のシリアル出力へ振り分ける
- `acs send`: 指定形式のダミーデータを継続してシリアルポートへ送信し、送信内容を monitor と同様のダッシュボードで表示する
- `--raw`: 改行でまとめず、生の受信チャンクをそのまま表示する
- `--display`: ポートごとに、受信・送信それぞれの表示形式を `hex` / `ascii` / `utf8` / `hex+ascii` / `hex+utf8` から選べる
- ログを、ローカル設定時は `./logs`、グローバル設定時は標準ユーザログディレクトリへ自動保存する
- `--config` による JSON 設定ファイル/ディレクトリの読み込みに対応し、指定がなければカレントディレクトリの `config/` と `acs.config.json` を優先し、見つからなければ標準ユーザ設定ディレクトリを読む
- `acs route` は組み込みテンプレートや設定ファイル内テンプレートを選んで簡単にルーティング構成を切り替えられる

## コマンド例

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
acs control --monitor /dev/ttyUSB1
acs control --raw --monitor /dev/ttyUSB1
acs control --display input:/dev/ttyUSB0=utf8 --display output:/dev/ttyUSB0=hex
acs monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1
acs monitor --raw --port /dev/ttyUSB0
acs monitor --display input:/dev/ttyUSB0=utf8 --display input:default=hex+utf8
acs route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
acs send --port /dev/ttyUSB0 --format PacketACv6
acs send --port /dev/ttyUSB0 --format PacketJFv1
acs send --config config
acs route --list-templates
acs route --config config
acs control --config config
```

接続されている DUALSHOCK 4 コントローラーが 1 台だけ、または使用可能なシリアルポートが 1 つだけの場合は、`acs` が自動で選択します。

`control`、`monitor`、`route` は、デフォルトでは受信データを改行単位でまとめて表示します。`--raw` を付けると、改行を待たずに受信チャンクをそのまま表示・記録します。

表示形式は `--display <TARGET>=<MODE>` で指定できます。`TARGET` には `PORT`、`input:PORT`、`output:PORT`、`default`、`input:default`、`output:default` が使えます。方向を付けない `PORT` や `default` は送受信の両方に適用されます。`MODE` には `hex`、`ascii`、`utf8`、`hex+ascii`、`hex+utf8` が使えます。指定しない場合は `hex+utf8` です。

## 初期化とローカル実行

Rust 関連がまっさらな状態からこのリポジトリを使い始めるなら、まずルートで次を実行してください。

```bash
make init
./acs --help
```

`make init` は `rustup` / `cargo` / stable toolchain を揃え、`release` ビルドまで実行します。このディレクトリ内では、以後 `./acs ...` でローカルランチャーとして使えます。

```bash
./acs control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
./acs monitor --port /dev/ttyUSB0
./acs route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
./acs send --port /dev/ttyUSB0 --format PacketACv6
```

直接 `cargo` を使いたい場合は従来どおり次でも動きます。

```bash
cargo run --release -- control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
cargo build --release
./target/release/acs monitor --port /dev/ttyUSB0
```

## グローバルインストールと更新

他のディレクトリからも `acs` をそのまま使いたい場合は、ルートで次を実行してください。

```bash
make install
```

通常は `~/.cargo/bin/acs` にインストールされます。`make install` はバイナリのインストールに加えて、標準ユーザ設定ディレクトリとログディレクトリも用意します。

- macOS の設定ディレクトリ: `~/Library/Application Support/acs`
- macOS のログディレクトリ: `~/Library/Logs/acs`
- Linux の設定ディレクトリ: `$XDG_CONFIG_HOME/acs` または `~/.config/acs`
- Linux のログディレクトリ: `$XDG_STATE_HOME/acs/logs` または `~/.local/state/acs/logs`

設定ディレクトリには、`config.example/` の JSON を「未作成のものだけ」コピーします。すでに編集済みの設定は上書きしません。

ローカルでコードを直したあと、そのチェックアウト内容をグローバルの `acs` バイナリへ反映したい場合は次を使えます。

```bash
make sync-code
```

`make sync-code` は、現在のチェックアウトからグローバルの `acs` バイナリを再インストールします。設定ファイルの投入は行わず、既存のグローバル設定はそのまま残します。

このリポジトリで調整したローカル設定をグローバル設定へ反映したい場合は、次を使えます。

```bash
make sync-config
```

`make sync-config` は、ローカルの `config/` があればそれを優先し、無ければ `acs.config.json` を使って、標準ユーザ設定ディレクトリ配下の `zz-local/` オーバーレイへ同期します。`zz-local/` は通常のグローバル設定より後で読まれるので、同じ `id` やキーを持つ設定を自然に上書きできます。

同期したローカル設定オーバーレイを外したい場合は次です。

```bash
make unsync-config
```

`PATH` が通っていれば、どのディレクトリからでも次のように実行できます。

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
acs monitor --port /dev/ttyUSB0
acs route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
acs send --port /dev/ttyUSB0 --format PacketACv6
```

アンインストールは次です。これはバイナリだけ削除し、設定とログは残します。

```bash
make uninstall
```

設定とログも含めて消したい場合は次を使います。

```bash
make purge
```

GitHub 上の tag を使ってグローバル版を更新したい場合は、次のどれかを使います。

```bash
make update
make update-latest
make update VERSION=v0.1.0
```

- `make update`: GitHub の tag 一覧から対話的に選んで更新
- `make update-latest`: 最新 tag で更新
- `make update VERSION=...`: 指定 tag で更新

## 設定ファイル

`--config` を省略した場合は、カレントディレクトリにある `config/` ディレクトリを自動で読み込みます。`config/` が無い場合だけ、後方互換として `acs.config.json` を探し、それも無い場合は標準ユーザ設定ディレクトリを探します。`--config` には JSON ファイルだけでなくディレクトリも指定できます。

`config/` を使う場合は、配下の `*.json` がファイル名順で順に読み込まれます。後から読まれたファイルほど優先され、`route.inputs` / `route.outputs` / `route.pipelines` / `route.templates` は `id` 単位で上書きできます。ログ出力先は、ローカル設定を使っているときは `./logs`、標準ユーザ設定ディレクトリを使っているときは標準ユーザログディレクトリが既定値です。

分割例は [config.example](config.example)、単一ファイル例は [acs.config.example.json](acs.config.example.json) を参照してください。

### `acs route` テンプレート

`acs route` では、`route <template>` または `route --template <name>` でルーティングテンプレートを選べます。`route.template` を設定しておけば、`acs route` 単体でもそのテンプレートを既定値として使えます。組み込みテンプレートは次の 2 つだけにしています。

- `merge`: 複数入力を来た順にそのまま全出力へ流す。出力が 1 つなら「2入力を来た順に1出力」になる
- `one-to-one`: 入力配列順と出力配列順を 1 対 1 に対応させ、その組だけにそのまま流す。余った input/output は無視する

一覧は次で確認できます。

```bash
acs route --list-templates
```

たとえば 2 入力を来た順に 1 出力へ流したいなら、次のように書けます。

```bash
acs route merge -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_main=/dev/ttyUSB2
```

入力と出力を順番に 1 対 1 対応させたいなら、次のように書けます。

```bash
acs route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3
```

```json
{
  "log_dir": "logs",
  "control": {
    "port": "/dev/ttyUSB0",
    "baud": 115200,
    "controller": "0",
    "format": "packetacv6",
    "raw": false,
    "display": {
      "default": "hex+utf8",
      "input": {
        "default": "utf8",
        "/dev/ttyUSB0": "utf8"
      },
      "output": {
        "default": "hex",
        "/dev/ttyUSB0": "hex"
      }
    },
    "monitor_ports": ["/dev/ttyUSB1"]
  },
  "send": {
    "port": "/dev/ttyUSB0",
    "baud": 115200,
    "format": "packetacv6",
    "display": {
      "output": {
        "default": "hex"
      }
    }
  },
  "monitor": {
    "ports": ["/dev/ttyUSB0", "/dev/ttyUSB1"],
    "baud": 115200,
    "raw": false,
    "display": {
      "default": "hex+utf8",
      "input": {
        "default": "hex+utf8",
        "/dev/ttyUSB0": "utf8"
      }
    }
  },
  "route": {
    "baud": 115200,
    "raw": true,
    "inputs": [
      { "id": "in_a", "port": "/dev/ttyUSB0" },
      { "id": "in_b", "port": "/dev/ttyUSB1" }
    ],
    "outputs": [
      { "id": "out_main", "port": "/dev/ttyUSB2" },
      { "id": "out_sub", "port": "/dev/ttyUSB3" }
    ],
    "pipelines": [
      {
        "id": "merge_passthrough",
        "inputs": ["in_a", "in_b"],
        "filter": { "module": "allow_all" },
        "transform": { "module": "identity" },
        "classify": { "module": "by_source" },
        "route": {
          "module": "broadcast",
          "outputs": ["out_main"]
        }
      }
    ],
    "templates": {
      "merge_csv": {
        "description": "latest payloads joined with comma",
        "pipelines": [
          {
            "id": "merge_csv",
            "transform": {
              "module": "join_latest",
              "separator_hex": "2c"
            },
            "route": {
              "module": "broadcast"
            }
          }
        ]
      }
    }
  }
}
```

コマンドライン引数を指定した場合は、設定ファイルの値よりもコマンドライン引数が優先されます。
