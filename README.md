# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

現在の第一段階で実装されている機能は以下のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- ログを `./logs` 以下へ自動保存する
- `--config` による JSON 設定ファイルの読み込みに対応する

## コマンド例

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format arm9
acs control --monitor /dev/ttyUSB1
acs monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1
acs control --config acs.config.example.json
```

接続されている DUALSHOCK 4 コントローラーが 1 台だけ、または使用可能なシリアルポートが 1 つだけの場合は、`acs` が自動で選択します。

## このディレクトリ内で実行する方法

このリポジトリのディレクトリ内では、まず `cargo run` でそのまま実行できます。

```bash
cargo run -- control --port /dev/ttyUSB0 --baud 115200 --format arm9
cargo run -- monitor --port /dev/ttyUSB0
```

一度ビルドしてから実行したい場合は、次のようにします。

```bash
cargo build
./target/debug/acs control --port /dev/ttyUSB0 --baud 115200 --format arm9
```

配布用や普段使い用に最適化ビルドしたい場合は `--release` を使います。

```bash
cargo build --release
./target/release/acs monitor --port /dev/ttyUSB0
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

設定例は [acs.config.example.json](acs.config.example.json) を参照してください。

```json
{
  "log_dir": "logs",
  "control": {
    "port": "/dev/ttyUSB0",
    "baud": 115200,
    "controller": "0",
    "format": "arm9",
    "monitor_ports": ["/dev/ttyUSB1"]
  },
  "monitor": {
    "ports": ["/dev/ttyUSB0", "/dev/ttyUSB1"],
    "baud": 115200
  }
}
```

コマンドライン引数を指定した場合は、設定ファイルの値よりもコマンドライン引数が優先されます。
