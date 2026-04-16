# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

現在の第一段階で実装されている機能は以下のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- `acs route`: 1 つ以上のシリアル入力を、設定に応じて 1 つ以上のシリアル出力へ振り分ける
- `acs send`: 指定形式のダミーデータを継続してシリアルポートへ送信し、送信内容と追加 monitor ポートの受信内容を同じダッシュボードで表示する
- `--raw`: 改行でまとめず、生の受信チャンクをそのまま表示する
- `--display`: ポートごとに、受信・送信それぞれの表示形式を `hex` / `ascii` / `utf8` / `hex+ascii` / `hex+utf8` から選べる
- ログを、ローカル設定時は `./logs`、グローバル設定時は標準ユーザログディレクトリへ自動保存する
- `--config` による JSON 設定ファイル/ディレクトリの読み込みに対応し、指定がなければカレントディレクトリの `config/` と `acs.config.json` を優先し、見つからなければ標準ユーザ設定ディレクトリを読む
- `acs route` は組み込みテンプレートや設定ファイル内テンプレートを選んで簡単にルーティング構成を切り替えられる

## コマンド例

```bash
acs control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
acs control --monitor /dev/ttyUSB1
acs control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8
acs control --raw --monitor /dev/ttyUSB1
acs control --display input:/dev/ttyUSB0=utf8 --display output:/dev/ttyUSB0=hex
acs monitor --port /dev/ttyUSB0@921600,utf8 --port /dev/ttyUSB1@115200,hex
acs monitor --raw --port /dev/ttyUSB0
acs monitor --display input:/dev/ttyUSB0=utf8 --display input:default=hex+utf8
acs route merge -i in_a=/dev/ttyUSB0@921600,utf8 -o out_main=/dev/ttyUSB1@115200,hex
acs send --port /dev/ttyUSB0@921600,hex --format PacketACv6
acs send --port /dev/ttyUSB0 --format PacketJFv1
acs send --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8
acs send --config config
acs route --list-templates
acs route --config config
acs control --config config
```

接続されている DUALSHOCK 4 コントローラーが 1 台だけ、または使用可能なシリアルポートが 1 つだけの場合は、`acs` が自動で選択します。

`control`、`monitor`、`route` は、デフォルトでは受信データを改行単位でまとめて表示します。`--raw` を付けると、改行を待たずに受信チャンクをそのまま表示・記録します。

表示形式は `--display <TARGET>=<MODE>` で指定できます。`TARGET` には `PORT`、`input:PORT`、`output:PORT`、`default`、`input:default`、`output:default` が使えます。方向を付けない `PORT` や `default` は送受信の両方に適用されます。`MODE` には `hex`、`ascii`、`utf8`、`hex+ascii`、`hex+utf8` が使えます。指定しない場合は `hex+utf8` です。

各 port 引数は `PORT[@BAUD][,DISPLAY]` の書式も使えます。`control --port` と `send --port` は output 側、`monitor --port` と `--monitor` は input 側、`route` は `ID=PORT[@BAUD][,DISPLAY]` でそれぞれの向きに適用されます。`--baud` は inline で `@BAUD` を書かなかった port の既定値です。

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
./acs route merge -i in_a=/dev/ttyUSB0@921600,utf8 -o out_main=/dev/ttyUSB1@115200,hex
./acs send --port /dev/ttyUSB0@921600,hex --format PacketACv6
./acs send --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8
```

直接 `cargo` を使いたい場合は従来どおり次でも動きます。

```bash
cargo run --release -- control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6
cargo build --release
./target/release/acs monitor --port /dev/ttyUSB0
```

release ビルドだけをしたい場合は次も使えます。

```bash
make build-release
```

## グローバルインストールと更新

他のディレクトリからも `acs` をそのまま使いたい場合は、ルートで次を実行してください。

```bash
make install
```

通常は `~/.cargo/bin/acs` にインストールされます。`make install` は、デフォルトではこのローカルリポジトリの「現在の HEAD commit の内容だけ」を使ってグローバルへ導入します。未コミット変更は取り込みません。

`make install` は初回導入用です。すでにグローバルに `acs` が入っている場合は再インストールせず、そのまま終了します。差し替えたい場合は `make update` または `make sync-code` を使ってください。

`make install` には ref 指定もできます。

```bash
make install TAG=v0.1.0
make install BRANCH=main
make install COMMIT=50d3137a75b821d46b5308f7c7693e513836e11d
```

`TAG` / `BRANCH` / `COMMIT` は GitHub 上の ref を使ってグローバル導入します。複数同時指定はできません。

インストール時には、build metadata として branch / commit / source kind / dirty state も埋め込みます。グローバル側では次で確認できます。

```bash
acs --version
acs version
```

tag と `Cargo.toml` の version を揃えてリリースしたい場合は、次で半自動化できます。

```bash
make release VERSION=1.2.0
```

`make release` は、clean な branch 上でだけ動きます。`Cargo.toml` と `Cargo.lock` の version を更新し、`cargo test`、`cargo build --release`、`git commit`、annotated tag の `v1.2.0` 作成まで実行します。push は自動では行わないので、最後に案内される `git push origin <branch> --follow-tags` を実行してください。

- macOS の設定ディレクトリ: `~/Library/Application Support/acs`
- macOS のログディレクトリ: `~/Library/Logs/acs`
- Linux の設定ディレクトリ: `$XDG_CONFIG_HOME/acs` または `~/.config/acs`
- Linux のログディレクトリ: `$XDG_STATE_HOME/acs/logs` または `~/.local/state/acs/logs`

設定ディレクトリには、`config.example/` の JSON を「未作成のものだけ」コピーします。すでに編集済みの設定は上書きしません。

ローカルでコードを直したあと、そのチェックアウト内容をグローバルの `acs` バイナリへ反映したい場合は次を使えます。

```bash
make sync-code
```

`make sync-code` は、現在の working tree からグローバルの `acs` バイナリを再インストールします。未コミット変更も取り込みます。反映された build が dirty な working tree 由来だった場合は、`acs --version` に `dirty` が出るので見分けられます。設定ファイルの投入は行わず、既存のグローバル設定はそのまま残します。

すでにグローバルに入っている `acs` を別の ref へ更新したい場合は次を使います。

```bash
make update
make update TAG=v0.1.0
make update BRANCH=main
make update COMMIT=50d3137a75b821d46b5308f7c7693e513836e11d
```

`make update` は、引数なしなら GitHub 上の最新 tag を使ってグローバル版を更新します。`TAG` / `BRANCH` / `COMMIT` を指定した場合は、その ref をソースとして更新します。

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
acs route merge -i in_a=/dev/ttyUSB0@921600,utf8 -o out_main=/dev/ttyUSB1@115200,hex
acs send --port /dev/ttyUSB0@921600,hex --format PacketACv6
acs send --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8
acs --version
```

アンインストールは次です。これはバイナリだけ削除し、設定とログは残します。

```bash
make uninstall
```

設定とログも含めて消したい場合は次を使います。

```bash
make purge
```

## 設定ファイル

`--config` を省略した場合は、カレントディレクトリにある `config/` ディレクトリを自動で読み込みます。`config/` が無い場合だけ、後方互換として `acs.config.json` を探し、それも無い場合は標準ユーザ設定ディレクトリを探します。`--config` には JSON ファイルだけでなくディレクトリも指定できます。

`config/` を使う場合は、配下の `*.json` がファイル名順で順に読み込まれます。後から読まれたファイルほど優先され、`route.inputs` / `route.outputs` / `route.pipelines` / `route.templates` は `id` 単位で上書きできます。ログ出力先は、ローカル設定を使っているときは `./logs`、標準ユーザ設定ディレクトリを使っているときは標準ユーザログディレクトリが既定値です。

分割例は [config.example](config.example)、単一ファイル例は [acs.config.example.json](acs.config.example.json) を参照してください。

port 設定は従来どおり文字列でも書けますが、個別のボーレートや表示形式を持たせたい場合は object 形式も使えます。`control.port` / `send.port` は `{ "path": "...", "baud": 921600, "display": "hex" }`、`monitor.ports` / `control.monitor_ports` / `send.monitor_ports` は同じ形の配列、`route.inputs` / `route.outputs` は既存の object に `"baud"` と `"display"` を追加できます。

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
acs route merge -i in_a=/dev/ttyUSB0@921600,utf8 -i in_b=/dev/ttyUSB1@115200,hex+ascii -o out_main=/dev/ttyUSB2@460800,hex
```

入力と出力を順番に 1 対 1 対応させたいなら、次のように書けます。

```bash
acs route one-to-one -i in_a=/dev/ttyUSB0@921600,utf8 -i in_b=/dev/ttyUSB1@115200,hex+ascii -o out_a=/dev/ttyUSB2@460800,hex -o out_b=/dev/ttyUSB3@115200,hex+ascii
```

```json
{
  "log_dir": "logs",
  "control": {
    "port": {
      "path": "/dev/ttyUSB0",
      "baud": 921600,
      "display": "hex"
    },
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
    "monitor_ports": [
      { "port": "/dev/ttyUSB1", "baud": 115200, "display": "utf8" }
    ]
  },
  "send": {
    "port": {
      "path": "/dev/ttyUSB0",
      "baud": 460800,
      "display": "hex"
    },
    "baud": 115200,
    "format": "packetacv6",
    "monitor_ports": [
      { "port": "/dev/ttyUSB2", "baud": 115200, "display": "utf8" }
    ],
    "display": {
      "output": {
        "default": "hex"
      }
    }
  },
  "monitor": {
    "ports": [
      { "path": "/dev/ttyUSB0", "baud": 921600, "display": "utf8" },
      { "path": "/dev/ttyUSB1", "baud": 115200, "display": "hex+ascii" }
    ],
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
      { "id": "in_a", "port": "/dev/ttyUSB0", "baud": 921600, "display": "utf8" },
      { "id": "in_b", "port": "/dev/ttyUSB1", "baud": 115200, "display": "hex+ascii" }
    ],
    "outputs": [
      { "id": "out_main", "port": "/dev/ttyUSB2", "baud": 460800, "display": "hex" },
      { "id": "out_sub", "port": "/dev/ttyUSB3", "baud": 115200, "display": "hex+ascii" }
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
