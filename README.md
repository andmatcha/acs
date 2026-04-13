# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

現在の第一段階で実装されている機能は以下のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- `acs route`: 1 つ以上のシリアル入力を、設定に応じて 1 つ以上のシリアル出力へ振り分ける
- `--raw`: 改行でまとめず、生の受信チャンクをそのまま表示する
- `--display`: ポートごとに、受信・送信それぞれの表示形式を `hex` / `ascii` / `utf8` / `hex+ascii` / `hex+utf8` から選べる
- ログを `./logs` 以下へ自動保存する
- `--config` による JSON 設定ファイルの読み込みに対応し、指定がなければカレントディレクトリの `acs.config.json` を自動で読む

## コマンド例

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format arm9
acs control --monitor /dev/ttyUSB1
acs control --raw --monitor /dev/ttyUSB1
acs control --display input:/dev/ttyUSB0=utf8 --display output:/dev/ttyUSB0=hex
acs monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1
acs monitor --raw --port /dev/ttyUSB0
acs monitor --display input:/dev/ttyUSB0=utf8 --display input:default=hex+utf8
acs route -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
acs route --config acs.config.json
acs control --config acs.config.json
```

接続されている DUALSHOCK 4 コントローラーが 1 台だけ、または使用可能なシリアルポートが 1 つだけの場合は、`acs` が自動で選択します。

`control`、`monitor`、`route` は、デフォルトでは受信データを改行単位でまとめて表示します。`--raw` を付けると、改行を待たずに受信チャンクをそのまま表示・記録します。

表示形式は `--display <TARGET>=<MODE>` で指定できます。`TARGET` には `PORT`、`input:PORT`、`output:PORT`、`default`、`input:default`、`output:default` が使えます。方向を付けない `PORT` や `default` は送受信の両方に適用されます。`MODE` には `hex`、`ascii`、`utf8`、`hex+ascii`、`hex+utf8` が使えます。指定しない場合は `hex+utf8` です。

## このディレクトリ内で実行する方法

このリポジトリのディレクトリ内では、まず `cargo run` でそのまま実行できます。

```bash
cargo run -- control --port /dev/ttyUSB0 --baud 115200 --format arm9
cargo run -- monitor --port /dev/ttyUSB0
cargo run -- route -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
```

一度ビルドしてから実行したい場合は、次のようにします。

```bash
cargo build
./target/debug/acs control --port /dev/ttyUSB0 --baud 115200 --format arm9
./target/debug/acs route -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
```

配布用や普段使い用に最適化ビルドしたい場合は `--release` を使います。

```bash
cargo build --release
./target/release/acs monitor --port /dev/ttyUSB0
./target/release/acs route -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
```

## グローバルで使えるようにする方法

`acs` を他のディレクトリからもそのまま使いたい場合は、このリポジトリのルートで以下を実行してください。

```bash
cargo install --path .
```

これで通常は `~/.cargo/bin/acs` にインストールされます。`~/.cargo/bin` が `PATH` に入っていれば、どのディレクトリからでも次のように実行できます。

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format arm9
acs monitor --port /dev/ttyUSB0
acs route -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1
```

`PATH` が通っていない場合は、シェル設定ファイルに追加してください。`zsh` なら例えば以下です。

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

ソースコードを更新したあとにグローバルインストールを上書きしたい場合は、次を使います。

```bash
cargo install --path . --force
```

## 設定ファイル

`--config` を省略した場合は、カレントディレクトリにある `acs.config.json` を自動で読み込みます。明示的に別の設定ファイルを使いたい場合だけ `--config` を指定してください。

設定例は [acs.config.example.json](acs.config.example.json) を参照してください。

```json
{
  "log_dir": "logs",
  "control": {
    "port": "/dev/ttyUSB0",
    "baud": 115200,
    "controller": "0",
    "format": "arm9",
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
    ]
  }
}
```

コマンドライン引数を指定した場合は、設定ファイルの値よりもコマンドライン引数が優先されます。
