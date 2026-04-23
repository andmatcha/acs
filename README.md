# acs

`acs` は ARES Project で使うコマンド群をまとめた ARES Command Set です。

主なコマンドは次のとおりです。

- `acs control`: DUALSHOCK 4 の入力を読み取り、整形したシリアル出力を送信する
- `acs monitor`: 1 つ以上のシリアルポートを監視する
- `acs route`: シリアル入力を built-in template に応じてシリアル出力へ振り分ける
- `acs send`: 指定形式のダミーデータを継続してシリアルポートへ送信する
- `acs xbee-mock`: `xbee-test` の片側だけを up/down 明示の port binding で実行する。`PAIR=1` なら 1 port 共用、`PAIR=2` なら uplink/downlink を分けて、実機や別プロセスの peer と組み合わせて片側だけの traffic model を流せる
- `acs xbee-rtt`: XBee 1 ペアに対して対称な疎通確認と RTT 計測を行う。通常は 2 台の PC で同じコマンドを 1 port ずつ使って実行し、`-p/--port` を 2 個指定したときだけ 1 台の PC 上で 2 個の XBee モジュールを相手にして測定する
- `acs xbee-test`: `base` / `remote` の 2 ポート間で AU(PacketACv6) / RU(RoverUpGeneral) と AD(PacketJFv1) / RD(RoverDownGeneral) の往復試験を行う。`flood` / `ping-pong` / `polling` を切り替えられ、ヘッダに実際の表示更新 fps も表示する

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

## 基本的な使い方

まずは全体のヘルプを確認してください。

```bash
acs --help
acs help control
acs help monitor
acs help route
acs help send
acs help xbee-mock
acs help xbee-rtt
acs help xbee-test
```

よく使う基本コマンドは次のとおりです。

```bash
acs ports
acs controllers
acs control --port /dev/ttyUSB0@921600 --config FORMAT=PacketACv6
acs monitor --port /dev/ttyUSB0@921600 --no-log
acs route merge -i in_a=/dev/ttyUSB0@921600 -o out_main=/dev/ttyUSB1@921600 --no-log
acs send --port /dev/ttyUSB0@921600 --config FORMAT=PacketJFv1,RATE=100
acs send --port /dev/ttyUSB0@921600 --config FORMAT=PacketACv6 --no-log
acs send -o ac=/dev/ttyUSB0@921600,hex,packetacv6,100 -o up=/dev/ttyUSB0@921600,utf8,roverupgeneral,10
acs send --port /dev/ttyUSB0@921600 --monitor /dev/ttyUSB1@115200,packetacv6+packetjfv1 --config FORMAT=PacketACv6
acs xbee-mock base -p /dev/ttyUSB0@921600 --config PAIR=1,TX_FORMAT=packetacv6@100+roverupgeneral@20,RX_FORMAT=packetjfv1+roverdowngeneral,TRAFFIC_PATTERN=flood
acs xbee-mock base -p up=/dev/ttyUSB0@921600 -p down=/dev/ttyUSB1@921600 --config PAIR=2,TX_FORMAT=packetacv6@100+roverupgeneral@20,RX_FORMAT=packetjfv1+roverdowngeneral,TRAFFIC_PATTERN=ping-pong
acs xbee-mock remote -p up=/dev/ttyUSB0@921600 -p down=/dev/ttyUSB1@921600 --config PAIR=2,TX_FORMAT=pollresponse@100,RX_FORMAT=pollgreeting,TRAFFIC_PATTERN=polling
acs xbee-rtt --port /dev/ttyUSB0
acs xbee-rtt --port /dev/ttyUSB0 --show-wire
acs xbee-rtt --port /dev/ttyUSB0 --show-protocol
acs xbee-rtt --port /dev/ttyUSB0@921600 --config PAYLOAD_SIZE=64,COUNT=20,INTERVAL_MS=50
acs xbee-rtt --port /dev/ttyUSB0@921600 --port /dev/ttyUSB1@921600 --config PAYLOAD_SIZE=64,COUNT=20,INTERVAL_MS=50
acs xbee-test --port base=/dev/ttyUSB0@921600 --port remote=/dev/ttyUSB1@921600 --config AU_RATE=100,RU_RATE=100,AD_RATE=100,RD_RATE=100
acs xbee-test --port base=/dev/ttyUSB0@921600 --port remote=/dev/ttyUSB1@921600 --config MODE=ping-pong,AU_RATE=100,RU_RATE=50
acs xbee-test --port base=/dev/ttyUSB0@921600 --port remote=/dev/ttyUSB1@921600 --config MODE=polling,POLL_RATE=100,BASE_REAL_PERCENT=10,REMOTE_REAL_PERCENT=20
acs --version
```

- 1 台だけコントローラーやシリアルポートが見つかる場合は、自動選択されます。
- すべてのポート指定で、ボーレート省略時は `115200` が使われます。
- 非 boolean の設定は、`--config KEY=VALUE,...` にまとめて指定できるコマンドが増えています。既存の個別フラグも互換のため引き続き受け付けます。
- `acs ports` の `[0]`, `[1]`, ... の番号は、`--port` / `--monitor` / `-i` / `-o` などのポート指定でそのまま使えます。
- 詳しいオプションや表示形式は `acs help <command>` を参照してください。
- `acs xbee-mock` は `PAIR=1` で 1 port を共用し、`PAIR=2` で uplink/downlink を分離できます。
- `acs xbee-mock` は `TX_FORMAT` に rate を持たせ、`RX_FORMAT` は monitor/decoder 対象 format を指定します。
- `acs xbee-rtt` は通常、各 PC で 1 個の XBee port を指定して同じコマンドを実行します。`--port` を 2 回指定したときだけ、1 プロセスでローカル 2 port を同時に動かします。
- `acs xbee-rtt` には `base` / `remote` の固定ロールはなく、hello nonce で測定順を対称に決めます。
- `acs xbee-rtt` は `payload-size` / `count` / `interval-ms` を省略すると、そのまま 32B を 10 回、100 ms 間隔で測定します。
- `acs xbee-rtt --show-wire` を付けると、実送受信の hex byte 列を青/赤で改行なしに垂れ流し表示します。1 台 PC のローカルペア時は `[0>]` / `[1<]` のような短い port ラベルも付きます。
- `acs xbee-rtt --show-protocol` を付けると、`HELLO(...)` / `PROBE(seq=3, ...)` / `RESULT(avg=...)` のような意味付きログを青/赤で改行なしに垂れ流し表示します。
- `acs xbee-rtt` は session ID と CRC16 付きの軽量バイナリフレームで再同期し、最終結果には payload bytes、回数、間隔、両方向の平均 RTT を表示します。
- `acs xbee-test` は表示更新が遅い場合も受信レートとエラー率の集計を優先し、packet 表示は別キューで追いかけます。
- `acs xbee-test` は `AU(PacketACv6) + RU(RoverUpGeneral)` と `AD(PacketJFv1) + RD(RoverDownGeneral)` をそれぞれ混在送信でき、各 format の送受信 Hz と照合結果を表示します。
- `acs xbee-test --config MODE=polling,...` は `PollGreeting` / `PollResponse` を基本にしつつ、`base` 側と `remote` 側で独立した確率で実パケット対へ差し替えます。
- `acs send` は同一ポートの mixed-format 受信でも packet を再同期し、format ごとの受信 Hz をヘッダに表示します。
- `acs send --monitor` は `packetacv6+packetjfv1` のように複数 format を指定でき、それぞれの built-in 既定表示で表示します。
- ログを保存する `acs control` / `acs monitor` / `acs route` / `acs send` / `acs xbee-test` / `acs xbee-mock` は、すべて `--no-log` でログファイル作成を止めて I/O 負荷を減らせます。

ログは既定で `./logs` に出力され、必要なら `--log-dir` か `--config LOG_DIR=...` で切り替えられます。

## その他の `make` コマンド

詳細は `make help` を見るのが早いです。よく使うものだけ挙げると次のとおりです。

- `make init`: Rust が未導入の環境を初期化し、ローカル release ビルドまで実行する
- `make build` / `make build-release`: ローカルでビルドする
- `make fmt` / `make test`: 整形とテストを実行する
- `make uninstall` / `make purge`: グローバルの `acs` を削除する
- `make paths`: 標準のログ・バイナリ配置先を表示する
- `make release VERSION=...`: バージョン更新、テスト、ビルド、コミット、タグ作成をまとめて行う
