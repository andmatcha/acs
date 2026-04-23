# `xbee-rtt` プロトコル仕様

## 対象

この文書は、`acs xbee-rtt` が 2026-04-23 JST 時点で使っている
対称・対等な XBee RTT 計測プロトコルをまとめた仕様書です。

実装のソースオブトゥルースは次です。

- `src/app/cli/xbee_rtt.rs`
- `src/output/formats/crc.rs`

## 前提

`xbee-rtt` の通常モードは、**2 台の PC がそれぞれ 1 個の XBee モジュールを持つ構成**です。

```text
PC-A -- serial -- XBee-A )))))) air link (((((( XBee-B -- serial -- PC-B
```

このとき、両方の PC で **同じ `acs xbee-rtt` コマンド**を実行します。
片側が `base`、もう片側が `remote` のような固定ロールはありません。

`--port` を 2 回指定したときだけ、1 プロセスが 2 本のローカル serial port を同時に開き、
2 個の XBee モジュールを **ローカルペア**として扱います。

```text
PC -- serial0 -- XBee-0 )))))) air link (((((( XBee-1 -- serial1 -- PC
```

wire protocol 自体はどちらのモードでも同じです。

## 結論

`xbee-rtt` は `xbee-rtt/2` という軽量バイナリプロトコルを使います。

- protocol name: `xbee-rtt/2`
- magic: `b"XR"`
- version: `2`
- endianness: little-endian
- CRC: `CRC16-CCITT-FALSE`
- 最大 payload 長: `4096 byte`

主な特徴は次です。

- どちらも同じ state machine で動く
- `HELLO` で 64 bit nonce と測定条件を交換する
- nonce の大小で、どちらが 1 本目を先に測るかを決める
- RTT 計測は 2 ラウンドあり、各ラウンドの initiator が 1 方向を測る
- 最後に `RESULT` を共有し、両側が両方向の測定値を持つ

## 用語

- `local`: その `acs xbee-rtt` プロセス自身
- `peer`: air link の向こう側の `acs xbee-rtt` プロセス
- `nonce`: 各 peer が起動時に生成する 64 bit ランダム値
- `session_id`: 2 つの nonce から対称に導出される 32 bit 値
- `round-1`: 小さい nonce 側が initiator になる計測ラウンド
- `round-2`: 大きい nonce 側が initiator になる計測ラウンド

## 既定値

CLI オプション未指定時は次で動きます。

- payload size: `32 byte`
- probe count: `10`
- probe interval: `100 ms`
- probe timeout: `1000 ms`
- connect timeout: `3000 ms`
- control retry interval: `200 ms`

## 実行条件

### 1-port / 2-PC モード

通常はこちらです。

```bash
acs xbee-rtt --port /dev/ttyUSB0
```

これを両方の PC で同じように実行します。

測定条件は `HELLO` で交換され、**一致していないとエラー**になります。

### 2-port / 1-PC ローカルペアモード

```bash
acs xbee-rtt --port /dev/ttyUSB0 --port /dev/ttyUSB1
```

このとき 1 プロセスが 2 本の local peer worker を起動します。
両 worker は通常モードと同じ protocol を独立に実行し、
最後に 2 本の結果を 1 つのペア結果へ統合します。

## フレーム全体

### レイアウト

フレームは次の可変長構造です。

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `magic` | 常に `b"XR"` |
| `2` | 1 | `u8` | `version` | 常に `2` |
| `3` | 1 | `u8` | `kind` | frame kind |
| `4..7` | 4 | `u32` | `session_id` | `HELLO` / `HELLO_ACK` では `0` |
| `8` | 1 | `u8` | `round_id` | `0`, `1`, `2` |
| `9..10` | 2 | `u16` | `seq` | probe 連番。control frame は通常 `0` |
| `11..12` | 2 | `u16` | `payload_len` | payload 長 |
| `13..N-3` | 可変 | bytes | `payload` | kind ごとの payload |
| `N-2..N-1` | 2 | `u16` | `crc16` | bytes `2..N-3` に対する `CRC16-CCITT-FALSE` |

補足:

- 固定 prefix は `13 byte`
- CRC を含む total overhead は `15 byte`
- `payload_len <= 4096` を満たさない frame は無効
- CRC は `magic` を含まない

### 再同期

stream からの切り出しは次の順で行います。

1. `XR` を探す
2. `version`, `kind`, `payload_len` を検証する
3. frame 全体がそろうまで待つ
4. CRC16 を検証する
5. 正常なら 1 frame として採用し、異常なら 1 byte ずらして再探索する

そのため、serial stream にノイズや途切れがあっても再同期できます。

## 列挙値

### `round_id`

| 値 | 意味 |
| ---: | --- |
| `0` | handshake / control phase |
| `1` | round-1 |
| `2` | round-2 |

### `kind`

| 値 | 名前 | 役割 |
| ---: | --- | --- |
| `1` | `HELLO` | nonce と測定条件の通知 |
| `2` | `HELLO_ACK` | `HELLO` を見た確認 |
| `3` | `MEASURE_START` | その round の開始通知 |
| `4` | `MEASURE_ACK` | `MEASURE_START` 応答 |
| `5` | `PROBE` | RTT 計測要求 |
| `6` | `PROBE_ECHO` | `PROBE` の即時エコー |
| `7` | `RESULT` | round 結果共有 |
| `8` | `RESULT_ACK` | `RESULT` 応答 |

## 各 payload

### `HELLO`

`session_id = 0`, `round_id = 0`, `seq = 0` です。

payload は 20 byte の `HelloWire` です。

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..7` | 8 | `u64` | `nonce` | sender の 64 bit nonce |
| `8..9` | 2 | `u16` | `payload_size` | probe payload 長 |
| `10..11` | 2 | `u16` | `probe_count` | 送信回数 |
| `12..15` | 4 | `u32` | `interval_ms` | probe 間隔 |
| `16..19` | 4 | `u32` | `probe_timeout_ms` | 1 probe の待ち時間 |

受信側の扱い:

- `nonce` を peer nonce として記録する
- 測定条件が local と一致しなければエラーにする
- `HELLO_ACK` を返す

### `HELLO_ACK`

`session_id = 0`, `round_id = 0`, `seq = 0` です。

payload は 8 byte の `HelloAckWire` です。

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..7` | 8 | `u64` | `ack_nonce` | ACK 対象の nonce |

受信側は `ack_nonce == 自分の nonce` のときだけ ACK 済みとみなします。

## session 確立

connectivity check 完了条件は次です。

- peer の `HELLO` を受信済み
- 自分の `HELLO` に対する `HELLO_ACK` を受信済み

この時点で双方は同じ 2 つの nonce を知っているので、
それらから対称に `session_id` を導出します。

### nonce collision

2 peer の nonce が完全一致した場合は collision とみなし、実装はエラー終了します。
確率は十分低いので、再実行で回避する前提です。

## initiator 決定

固定ロールはありません。

- `round-1` initiator = **小さい nonce 側**
- `round-2` initiator = **大きい nonce 側**

つまり、各 peer は必ず 1 回ずつ initiator になります。

## `MEASURE_START`

payload は 12 byte の `MeasurementSpecWire` です。

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `u16` | `payload_size` | probe payload 長 |
| `2..3` | 2 | `u16` | `probe_count` | 送信回数 |
| `4..7` | 4 | `u32` | `interval_ms` | probe 間隔 |
| `8..11` | 4 | `u32` | `probe_timeout_ms` | 1 probe の待ち時間 |

受信側は spec が local のものと一致することを確認し、`MEASURE_ACK` を返します。

### `MEASURE_ACK`

payload は空です。

initiator は ACK を受けるまで `200 ms` ごとに `MEASURE_START` を再送します。

## `PROBE`

payload は任意 byte 列です。

- 実装では `session_id`, `round_id`, `seq` を seed にした疑似乱数列を使う
- receiver は内容を解釈せず、そのまま返せばよい
- `seq` は `1` から始まる

## `PROBE_ECHO`

payload は `PROBE` の完全コピーです。

- `round_id` も `seq` も `PROBE` と同じ値
- responder は `PROBE` を受けたら即時返送する

initiator は次で判定します。

- 同じ `round_id` / `seq` の `PROBE_ECHO` が timeout 内に返る
- payload が完全一致する

結果:

- 一致したら success
- echo は来たが payload 不一致なら mismatch
- timeout まで echo が来なければ timeout

## `RESULT`

payload は 22 byte の `MeasurementSummaryWire` です。

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `u16` | `probe_count` | 実施予定 probe 数 |
| `2..3` | 2 | `u16` | `success_count` | 正常往復数 |
| `4..5` | 2 | `u16` | `timeout_count` | timeout 数 |
| `6..7` | 2 | `u16` | `mismatch_count` | mismatch 数 |
| `8..9` | 2 | `u16` | `payload_size` | probe payload 長 |
| `10..13` | 4 | `u32` | `mean_rtt_us` | 平均 RTT [us] |
| `14..17` | 4 | `u32` | `min_rtt_us` | 最小 RTT [us] |
| `18..21` | 4 | `u32` | `max_rtt_us` | 最大 RTT [us] |

補足:

- RTT は `PROBE` 送信時刻から対応する `PROBE_ECHO` 受信時刻まで
- 平均は success sample の平均を四捨五入した microsecond 値
- success が 0 件なら `mean/min/max` はすべて `0`

### `RESULT_ACK`

payload は空です。

initiator は ACK を受けるまで `200 ms` ごとに `RESULT` を再送します。

## 実行フロー

1 peer 視点での通常フローは次です。

1. port を開く
2. `HELLO` を periodic に送る
3. peer の `HELLO` を受けたら `HELLO_ACK` を返す
4. 自分の `HELLO_ACK` が返ってきたら connectivity check 完了
5. nonce の大小で `round-1` / `round-2` の initiator を決める
6. initiator になる round では `MEASURE_START` -> `PROBE/ECHO` -> `RESULT`
7. responder になる round では `MEASURE_ACK` / `PROBE_ECHO` / `RESULT_ACK`
8. 2 round 完了で終了

## 成功条件

各 peer は次の 2 つを最終結果として持ちます。

- `local -> peer` の RTT 測定結果
- `peer -> local` の RTT 測定結果

CLI 上の clean success は次を満たすときです。

- outbound result が clean
- inbound result が clean

つまり、

- `success_count == probe_count`
- timeout が 0
- mismatch が 0

が両方向で成り立つ必要があります。

## wire byte 数の目安

1 回の probe 往復で実際に air link を通る byte 数は、
`PROBE` と `PROBE_ECHO` の 2 frame 分です。

```text
round_trip_wire_bytes = (15 + payload_size) * 2
```

既定値 `payload_size = 32` の場合は次です。

```text
(15 + 32) * 2 = 94 byte
```

これは control frame の `HELLO` / `HELLO_ACK` / `MEASURE_START` / `MEASURE_ACK` /
`RESULT` / `RESULT_ACK` を除いた、純粋な 1 probe あたりの往復量です。

## 実装依存だが互換性に関わる点

### 無視する frame

実装は次を protocol error にせず無視します。

- `session_id` が現在の session と違う
- `round_id` が現在待っている round と違う
- `payload_len > 4096`
- `version` が `2` ではない
- 未知の `kind`
- CRC 不一致

### option mismatch

`HELLO` で受け取った `payload_size`, `count`, `interval_ms`, `probe_timeout_ms` が
local と違う場合、実装は即時にエラーで終了します。

つまり 2-PC モードでは、両側で同じ測定オプションを指定する前提です。

## 再実装時に守るべき点

最低限、次を守れば `acs xbee-rtt` と相互接続できます。

- `magic = XR`
- `version = 2`
- すべて little-endian
- CRC は `magic` を除く
- `HELLO` で 64 bit nonce と測定条件を送る
- `HELLO_ACK` は受け取った nonce を返す
- 小さい nonce 側が `round-1` initiator
- `PROBE_ECHO` は `round_id`, `seq`, `payload` をそのまま返す
- `RESULT_ACK` は `RESULT` を受理してから返す

この条件を満たせば、`1-port/2-PC` と `2-port/1-PC` のどちらでも `acs xbee-rtt` と接続できます。
