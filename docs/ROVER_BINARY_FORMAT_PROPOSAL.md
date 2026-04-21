# Rover 系 ASCII format のバイナリ圧縮案

## 結論

`RoverUpGeneral` / `RoverDownGeneral` は、文字列のままでも固定長で扱いやすい一方、帯域効率と型安全性の面ではまだかなり圧縮余地があります。

- 可能です。
- ただし、`1 行の ASCII を 1 フレームのバイナリへそのまま置き換える` だけだと圧縮率は限定的です。
- 本当に効くのは、`ID を辞書化` し、`数値を型付きバイナリ化` し、`複数レコードを 1 パケットへバッチ化` する案です。

`PacketACv6` に合わせて、少なくとも次を持たせるのがよいです。

- ヘッダー
- シーケンス番号
- バージョン / 種別
- CRC16

## 現状整理

実装上、現状の Rover 系 format は ASCII 固定長として認識されています。

### `RoverUpGeneral`

例:

```text
0x300,1234\r\n
```

- 長さ: 12 byte
- 構造: `0x` + 3 桁 hex ID + `,` + 4 桁 decimal value + `\r\n`

現行実装の例:

- [src/output/formats/roverupgeneral/mod.rs](/Users/jinaoyagi/workspace/ares/acs/src/output/formats/roverupgeneral/mod.rs)
- [src/app/cli/send.rs](/Users/jinaoyagi/workspace/ares/acs/src/app/cli/send.rs:683)

### `RoverDownGeneral`

例:

```text
400,21.10\r\n
```

- 長さ: 11 byte
- 構造: 3 桁 hex ID + `,` + `dd.dd` + `\r\n`

現行実装の例:

- [src/output/formats/roverdowngeneral/mod.rs](/Users/jinaoyagi/workspace/ares/acs/src/output/formats/roverdowngeneral/mod.rs)
- [src/app/cli/send.rs](/Users/jinaoyagi/workspace/ares/acs/src/app/cli/send.rs:712)

### 現状の利点

- 人間が目視しやすい
- ターミナル表示で扱いやすい
- 固定長なので簡易な切り出しはしやすい

### 現状の課題

- 数値が文字列化されるので無駄が多い
- `float` / `double` を文字列で送ると桁数次第でさらに肥大化する
- 整数 / 固定小数 / 浮動小数の区別が wire 上にない
- `PacketACv6` と違って checksum がない
- シーケンス番号がないので欠落・重複の追跡がしづらい
- 現状の再同期は「先頭文字パターン」と固定長に依存している

## `PacketACv6` から借りたい要素

`PacketACv6` は次の点が参考になります。

- 2 byte header で再同期しやすい
- sequence byte を持つ
- `CRC16-CCITT-FALSE` で payload 全体を検証する
- little-endian で統一されている

参考:

- [docs/PACKET_ACV6_PACKET_AND_CAN.md](/Users/jinaoyagi/workspace/ares/acs/docs/PACKET_ACV6_PACKET_AND_CAN.md)
- [src/output/formats/packetacv6/definition.rs](/Users/jinaoyagi/workspace/ares/acs/src/output/formats/packetacv6/definition.rs)
- [src/output/formats/packetacv6/encoder.rs](/Users/jinaoyagi/workspace/ares/acs/src/output/formats/packetacv6/encoder.rs)
- [src/output/formats/crc.rs](/Users/jinaoyagi/workspace/ares/acs/src/output/formats/crc.rs)

## 圧縮の基本方針

### 1. ID を文字列ではなく辞書 index にする

`0x300` や `0x412` を毎回 ASCII で送る必要はありません。

送受信の双方で `signal_code -> legacy_id` の対応表を共有し、wire 上では `u8` で送るのが最も効率的です。

例:

| `signal_code` | legacy ID | 方向 |
| ---: | --- | --- |
| `0x00` | `0x300` | up |
| `0x01` | `0x310` | up |
| `0x02` | `0x311` | up |
| `0x20` | `0x400` | down |
| `0x21` | `0x401` | down |
| `0x2B` | `0x412` | down |

これだけで、ID 部分は `ASCII 5 byte 前後` から `1 byte` にできます。

### 2. 値は文字列ではなく型付き数値で送る

推奨順は次のとおりです。

1. `u16` / `i16`
2. `i32` 固定小数点
3. `f32`
4. `f64` は本当に必要な信号だけ

#### 固定小数点を第一候補にする理由

- ASCII より短い
- `f32` / `f64` より比較しやすい
- 小数桁を signal ごとに固定できる
- 組み込み側で扱いやすい

例:

| 元の文字列 | 推奨内部表現 | wire 値 |
| --- | --- | --- |
| `21.10` | `q0.01` | `2110` |
| `-3.250` | `q0.001` | `-3250` |
| `1234.5678` | `q0.0001` | `12345678` |

#### `float` / `double` の扱い

現状の Rover 系では `float` / `double` も文字列化して送る運用があるとのことなので、次の 2 段構えを推奨します。

- 基本は `fixed-point`
- 本当に scale が固定できないものだけ `f32` または `f64`

特に `double` を ASCII で送ると 15 byte 以上になりやすく、圧縮効果はかなり大きくなります。一方で、精度要件が `f64` でない限り、通信仕様としては `f32` へ寄せた方が帯域効率はよいです。

### 3. 単発送信よりバッチ送信を前提にする

ここが一番重要です。

`PacketACv6` のように packet 単位で header / seq / CRC を付ける場合、1 レコードごとに完全な envelope を載せると overhead が相対的に大きくなります。

そのため、推奨は `1 signal = 1 packet` ではなく、

- `1 packet = 複数 signal`

です。

## 推奨仕様案: `RoverBinaryBatch v1`

### フレーム全体

`PacketACv6` と同様に header と CRC16 を持つ可変長 packet とします。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `header` | 常に `b"RV"` |
| `2` | 1 | `u8` | `version_and_kind` | 上位 4 bit: version、下位 4 bit: packet kind |
| `3` | 1 | `u8` | `seq` | 送信ごとに `wrapping_add(1)` |
| `4` | 1 | `u8` | `flags` | ACK 要求や圧縮種別など |
| `5` | 1 | `u8` | `record_count` | record 数 |
| `6..N-3` | 可変 | bytes | `records` | signal record 群 |
| `N-2..N-1` | 2 | `u16` | `crc16` | bytes `0..N-3` に対する `CRC16-CCITT-FALSE` |

### `packet kind`

| 値 | 意味 |
| ---: | --- |
| `0x0` | Up batch |
| `0x1` | Down batch |
| `0x2` | Mixed / extended batch |
| `0xF` | 将来拡張用 |

### エンディアン

- 数値は little-endian
- CRC も little-endian

`PacketACv6` と揃えることで、実装の再利用とデバッグの一貫性を保ちやすくなります。

## record の推奨構造

### A. 通常 record

通常は signal ごとに型を固定し、record は最小化します。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0` | 1 | `u8` | `signal_code` | signal 辞書 index |
| `1..M-1` | 2 / 4 / 8 | 数値 | `value` | signal 定義に応じた値 |

この方式では、型情報は wire に毎回載せず、`signal_code` 側の定義表で解決します。

例:

- `0x300` 系 command: `u16`
- `0x400` 系 telemetry: `i16(q0.01)` または `i32(q0.001)`
- どうしても必要なものだけ `f32` / `f64`

### B. 拡張 record

将来、1 つの batch に複数型を混在させたい場合だけ拡張 record を使います。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0` | 1 | `u8` | `signal_code` | signal 辞書 index |
| `1` | 1 | `u8` | `value_type` | `u16` / `i16_q2` / `i32_q3` / `f32` / `f64` など |
| `2` | 1 | `u8` | `value_len` | value byte 長 |
| `3..` | 可変 | bytes | `value` | 生 value |

ただし、この record は通常系より overhead が大きいので、常用よりも例外扱いがよいです。

## signal 定義表の持ち方

バイナリ化では、wire format だけでなく `signal definition table` を仕様として固定するのが重要です。

各 signal について最低でも次を持たせる想定です。

| 項目 | 例 |
| --- | --- |
| `signal_code` | `0x21` |
| `legacy_id` | `0x401` |
| `direction` | down |
| `encoding` | `i16_q0.01` |
| `unit` | `deg`, `A`, `mm`, `ratio` など |
| `valid_range` | `-5000..5000` など |

これを送受信双方で共有すれば、wire 上の情報量を大きく減らせます。

## 具体案: `RU` ヘッダー + 型付き可変長 payload + 終端マーカー

今回の条件に寄せると、より素直な 1 レコード置換案は次のようになります。

- header は ASCII で `b"RU"`
- ID は `0x300` や `0x412` を `u16` 化して格納する
  - 実際に使うのは下位 12 bit
  - wire 上は 2 byte とする
- payload は文字列を可能なら数値へ parse して格納する
- payload type は 1 byte の type tag で明示する
- payload は可変長
- packet 末尾には終端マーカーを置く

### 1 レコードの基本構造

CRC なしの最小構成は次です。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `header` | 常に `b"RU"` |
| `2..3` | 2 | `u16` | `id` | `0x300` などを little-endian で格納 |
| `4` | 1 | `u8` | `type_tag` | payload 型 |
| `5..N-2` | 可変 | bytes | `payload` | 型に応じた値 |
| `N-1` | 1 | `u8` | `end_marker` | 例: `0x03` (`ETX`) |

CRC16 を付けるなら、終端の直前に 2 byte 足します。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `header` | 常に `b"RU"` |
| `2..3` | 2 | `u16` | `id` | `0x300` などを little-endian で格納 |
| `4` | 1 | `u8` | `type_tag` | payload 型 |
| `5..N-4` | 可変 | bytes | `payload` | 型に応じた値 |
| `N-3..N-2` | 2 | `u16` | `crc16` | `CRC16-CCITT-FALSE` |
| `N-1` | 1 | `u8` | `end_marker` | 例: `0x03` (`ETX`) |

### type tag 例

| 値 | 意味 | payload byte 数 |
| ---: | --- | ---: |
| `0x01` | raw UTF-8 string | 可変 |
| `0x11` | `u16` | 2 |
| `0x12` | `i16` | 2 |
| `0x13` | `u32` | 4 |
| `0x14` | `i32` | 4 |
| `0x21` | `i16(q0.01)` | 2 |
| `0x22` | `i32(q0.001)` | 4 |
| `0x31` | `f32` | 4 |
| `0x32` | `f64` | 8 |

### この方式のサイズ式

CRC なし:

```text
packet_size = 2(header) + 2(id) + 1(type) + payload_bytes + 1(end)
            = payload_bytes + 6
```

CRC16 あり:

```text
packet_size = payload_bytes + 8
```

### 現行 ASCII との比較の見方

`RoverUpGeneral` の 1 行はおおむね次です。

```text
0xXYZ,<payload>\r\n
```

したがって、

```text
ascii_size_up = payload_chars + 8
```

`RoverDownGeneral` の 1 行はおおむね次です。

```text
XYZ,<payload>\r\n
```

したがって、

```text
ascii_size_down = payload_chars + 6
```

このため、今回の `RU` 方式は

- `Up` 系では `0x` と `,` と `\r\n` を減らせるぶん有利
- `Down` 系では ID がもともと短いので、`fixed-point` 化しないと効きにくい
- `CRC16` を付けると小さい payload では削減幅がかなり縮む

という傾向になります。

### 例 1: 整数 command

入力:

```text
0x300,1234\r\n
```

- ASCII 長: 12 byte
- parse 結果: `id=0x300`, `type=u16`, `value=1234`
- binary payload: 2 byte

RU packet:

```text
52 55 00 03 11 D2 04 03
```

- `52 55`: `RU`
- `00 03`: `0x300` little-endian
- `11`: `u16`
- `D2 04`: `1234`
- `03`: end marker

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 12 | - |
| `RU`、CRC なし | 8 | 33.3% 削減 |
| `RU`、CRC16 あり | 10 | 16.7% 削減 |

### 例 2: 小数 2 桁 telemetry を fixed-point 化

入力:

```text
400,21.10\r\n
```

- ASCII 長: 11 byte
- parse 結果: `id=0x400`, `type=i16(q0.01)`, `value=2110`
- binary payload: 2 byte

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 11 | - |
| `RU`、`i16(q0.01)`、CRC なし | 8 | 27.3% 削減 |
| `RU`、`i16(q0.01)`、CRC16 あり | 10 | 9.1% 削減 |
| `RU`、`f32`、CRC なし | 10 | 9.1% 削減 |
| `RU`、`f32`、CRC16 あり | 12 | 9.1% 増加 |

この例では、`21.10` を `f32` にするより `q0.01` の fixed-point にした方が明確に有利です。

### 例 3: 負値の小数

入力:

```text
401,-3.250\r\n
```

- ASCII 長: 12 byte
- parse 結果: `id=0x401`, `type=i16(q0.001)`, `value=-3250`
- binary payload: 2 byte

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 12 | - |
| `RU`、`i16(q0.001)`、CRC なし | 8 | 33.3% 削減 |
| `RU`、`i16(q0.001)`、CRC16 あり | 10 | 16.7% 削減 |
| `RU`、`f32`、CRC なし | 10 | 16.7% 削減 |
| `RU`、`f32`、CRC16 あり | 12 | 同等 |

### 例 4: 小数 4 桁を `i32` fixed-point 化

入力:

```text
410,1234.5678\r\n
```

- ASCII 長: 15 byte
- parse 結果: `id=0x410`, `type=i32(q0.0001)`, `value=12345678`
- binary payload: 4 byte

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 15 | - |
| `RU`、`i32(q0.0001)`、CRC なし | 10 | 33.3% 削減 |
| `RU`、`i32(q0.0001)`、CRC16 あり | 12 | 20.0% 削減 |
| `RU`、`f32`、CRC なし | 10 | 33.3% 削減 |
| `RU`、`f64`、CRC なし | 14 | 6.7% 削減 |

この程度の桁数なら、精度要件次第で `i32(q0.0001)` か `f32` が有力です。

### 例 5: `double` 相当の長い decimal

入力:

```text
411,0.123456789012345\r\n
```

- ASCII 長: 23 byte
- parse 結果: `id=0x411`, `type=f64`, `value=0.123456789012345`
- binary payload: 8 byte

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 23 | - |
| `RU`、`f64`、CRC なし | 14 | 39.1% 削減 |
| `RU`、`f64`、CRC16 あり | 16 | 30.4% 削減 |

ASCII の `double` 文字列は長くなりやすいので、この領域では binary 化の効果がかなり出ます。

### 例 6: 数値化できない文字列

入力:

```text
0x318,OPEN\r\n
```

- ASCII 長: 12 byte
- parse 結果: `id=0x318`, `type=raw_utf8`, `value=\"OPEN\"`
- binary payload: 4 byte

比較:

| 方式 | サイズ | ASCII 比 |
| --- | ---: | ---: |
| 現行 ASCII | 12 | - |
| `RU`、raw UTF-8、CRC なし | 10 | 16.7% 削減 |
| `RU`、raw UTF-8、CRC16 あり | 12 | 同等 |

文字列 payload は、ID と区切り文字の分だけは減りますが、数値化できるケースほどは効きません。

### ざっくりした傾向

| payload の種類 | 圧縮傾向 |
| --- | --- |
| `u16` / `i16` に落ちる整数 | 効きやすい |
| `q0.01` / `q0.001` に落ちる fixed-point | 効きやすい |
| 短い decimal を `f32` 化 | 効きにくい |
| 長い decimal を `f64` 化 | 効きやすい |
| 非数値の短い文字列 | 少しだけ効く |

### この `RU` 案の評価

この方式は、

- 現行の `1 行 = 1 レコード` モデルを大きく崩さずに移行しやすい
- 型を明示できる
- `0x300,1234\r\n` のような整数系では素直に削減できる

一方で、

- 小さい packet に `CRC16` まで載せると利得が薄くなる
- `f32` 化だけでは短い decimal にあまり効かない
- 可変長 payload に終端マーカーを使う場合、将来 arbitrary binary を入れるなら escape か length 併用が必要

という性質があります。

## 単発 `ID,DATA\r\n` を複数個溜めて packet 化する場合の分析

ここでは、単発で到着する `ID,DATA\r\n` を複数個バッファし、1 つの `RU` batch packet にまとめて送るケースを分析します。

### 前提

可変長 payload を batch に詰める場合、各 record の境界が必要なので、ここでは次の batch 形式を前提にします。

### batch frame

| フィールド | size |
| --- | ---: |
| `header = "RU"` | 2 |
| `seq` | 1 |
| `record_count` | 1 |
| `records` | 可変 |
| `crc16` | 2 |
| `end_marker` | 1 |

固定 overhead は 7 byte です。

### batch 内 record

| フィールド | size |
| --- | ---: |
| `id: u16` | 2 |
| `type_tag: u8` | 1 |
| `payload_len: u8` | 1 |
| `payload` | 可変 |

record サイズは `payload_bytes + 4` byte です。

### batch サイズ式

```text
batch_size = 7 + Σ(payload_bytes_i + 4)
```

型ごとの 1 record サイズは次の通りです。

| payload 型 | 1 record サイズ |
| --- | ---: |
| `u16` / `i16(q0.01)` | 6 byte |
| `i32(q0.001)` / `f32` | 8 byte |
| `f64` | 12 byte |
| raw string 長さ `L` | `L + 4` byte |

### 通信量節約効果

#### ケース A: Up 系の整数 command

例:

```text
0x300,1234\r\n
```

- ASCII 1 件: 12 byte
- batch record 1 件: 6 byte
- batch 全体: `7 + 6N`

| record 数 `N` | ASCII 合計 | batch 合計 | 削減率 |
| ---: | ---: | ---: | ---: |
| 1 | 12 | 13 | `8.3%` 増加 |
| 2 | 24 | 19 | `20.8%` 削減 |
| 4 | 48 | 31 | `35.4%` 削減 |
| 8 | 96 | 55 | `42.7%` 削減 |
| 10 | 120 | 67 | `44.2%` 削減 |

この形式では、`N=1` ではむしろ増えます。batch 化の利点が出るのは `2 件以上` からです。

#### ケース B: Down 系の小数 2 桁 telemetry を fixed-point 化

例:

```text
400,21.10\r\n
```

- ASCII 1 件: 11 byte
- batch record 1 件: 6 byte
- batch 全体: `7 + 6N`

| record 数 `N` | ASCII 合計 | batch 合計 | 削減率 |
| ---: | ---: | ---: | ---: |
| 1 | 11 | 13 | `18.2%` 増加 |
| 2 | 22 | 19 | `13.6%` 削減 |
| 4 | 44 | 31 | `29.5%` 削減 |
| 8 | 88 | 55 | `37.5%` 削減 |
| 12 | 132 | 79 | `40.2%` 削減 |

`Down` 系は元の ID 表記が短いので、`fixed-point` に落とせないと利得が出にくいです。

#### ケース C: 長い decimal を `f64` 化

例:

```text
411,0.123456789012345\r\n
```

- ASCII 1 件: 23 byte
- batch record 1 件: 12 byte
- batch 全体: `7 + 12N`

| record 数 `N` | ASCII 合計 | batch 合計 | 削減率 |
| ---: | ---: | ---: | ---: |
| 1 | 23 | 19 | `17.4%` 削減 |
| 2 | 46 | 31 | `32.6%` 削減 |
| 4 | 92 | 55 | `40.2%` 削減 |
| 8 | 184 | 103 | `44.0%` 削減 |

ASCII の `double` 文字列は長いので、`f64` でも十分効果があります。

### まとめ

- `u16` / `i16 fixed-point` に落とせる信号を複数件まとめると、40% 前後の削減が見込める
- `f64` が必要な長い decimal でも、複数件 batch ならやはり 40% 前後まで縮む
- 逆に `1 件だけ` を batch 容器に入れると、overhead のせいで増える場合がある

### リアルタイム性への影響

通信量は減りますが、batch は「溜めてから送る」ので待ち時間が増えます。リアルタイム性への影響は、ほぼこの待ち時間で決まります。

#### サイズ閾値で flush する場合

record 到着レートを `R [records/s]`、batch record 数を `N` とすると、

```text
到着間隔 Δ = 1 / R
最大追加待ち時間 = (N - 1) * Δ
平均追加待ち時間 ≒ (N - 1) * Δ / 2
```

例えば `R = 100 records/s` なら `Δ = 10 ms` です。

| batch record 数 `N` | 最大追加待ち時間 | 平均追加待ち時間 |
| ---: | ---: | ---: |
| 2 | 10 ms | 5 ms |
| 4 | 30 ms | 15 ms |
| 8 | 70 ms | 35 ms |
| 10 | 90 ms | 45 ms |
| 12 | 110 ms | 55 ms |

このため、`100 Hz` 近辺の制御で `N=8` 以上を待つ設計は、制御入力としてはかなり重くなります。

#### シリアル送信時間の削減との比較

UART 8N1 とすると、実線路上の送信時間はおおむね

```text
line_time_ms ≒ bytes * 10 / baud * 1000
```

です。

1 byte あたりの線路時間は次の通りです。

| baud | 1 byte の送信時間 |
| ---: | ---: |
| `115200` | `0.0868 ms` |
| `921600` | `0.0109 ms` |

`Up` 整数 10 件の例:

- ASCII: 120 byte
- batch: 67 byte

| baud | ASCII 線路時間 | batch 線路時間 | 節約できる線路時間 |
| ---: | ---: | ---: | ---: |
| `115200` | `10.42 ms` | `5.82 ms` | `4.60 ms` |
| `921600` | `1.30 ms` | `0.73 ms` | `0.57 ms` |

重要なのは、`N=10` の batch を作るために `100 Hz` ストリームで待つと、最大 `90 ms` の待ちが増える一方、線路時間の節約は `115200` でも `4.6 ms` 程度しかないことです。

つまり、

- 通信量は減る
- しかし、均等到着する単発 stream を後から溜めると、遅延増加の方がかなり大きい

というのが基本です。

#### batch 化が向くケース

- もともと同一周期で複数 signal をまとめて生成している
- 1 制御 tick の snapshot をまとめて送れる
- 数十 ms の追加遅延が許容される telemetry 主体
- 低 baud で帯域が厳しい

#### batch 化が向かないケース

- 単発 command を届き次第すぐ反映したい
- 古い値と新しい値が混じると意味が変わる
- `100 Hz` 以上の応答性が欲しい
- 各 record が到着時刻に意味を持つ

#### 時刻ずれの問題

単発 record を溜めてから送ると、同じ packet に入っていても oldest と newest の生成時刻がずれます。

例えば `100 records/s` で `N=10` なら、1 packet 内で最大 `90 ms` の age 差があり得ます。

受信側が packet 到着時刻だけを見て「同時刻の snapshot」と解釈すると危険です。これを避けるには次のいずれかが必要です。

- 1 周期内で同時取得した値だけを batch 化する
- batch に共通 timestamp を持たせる
- さらに必要なら record ごとの age / delta を持たせる

### エンコード処理時間

送信側のエンコードは、ざっくり次の処理です。

1. `ID` の 16 進 3 桁を parse
2. `DATA` 文字列を数値にできるか判定
3. `u16` / fixed-point / `f32` / `f64` のいずれかへ変換
4. batch buffer へ追記
5. flush 時に CRC16 を計算

計算量は概ね

```text
O(入力文字数合計 + 出力 byte 数合計)
```

です。

#### 実務上の見え方

- 整数や `q0.01` への変換は軽い
- `f32` / `f64` への decimal parse は相対的に重い
- CRC16 は packet が 50 - 100 byte 程度なら軽い

`Up` 整数 10 件の batch なら、

- 入力文字数は 120 byte 程度
- 出力は 67 byte
- CRC16 対象も 67 byte 程度

なので、PC クラスでは通常はシリアル送信時間より十分小さいはずです。

ただし、`double` 文字列の parse は整数より重いので、`float` / `double` を大量に扱うなら sender 側 CPU 使用率は目立ちやすくなります。

### デコード処理時間

受信側デコードは、ASCII よりむしろ軽くなることが多いです。

処理は次です。

1. `RU` header を検出
2. `record_count` 分だけ順に走査
3. `id`, `type_tag`, `payload_len` を読む
4. payload を native 値として復元
5. CRC16 を検証

ASCII 受信時のような

- 区切り文字探索
- decimal / float 文字列 parse

が不要になるため、受信側がそのまま数値を使うなら decode はかなり素直です。

#### CPU 負荷の移り方

- sender: 増える
- receiver: 減る

特に「受信側は値を数値として使いたいだけ」の場合、全体 CPU としては改善しやすいです。

逆に、受信側でも最終的に ASCII 表示へ戻すなら、その分の再文字列化コストが追加されます。

### 総合評価

単発 `ID,DATA\r\n` を後段で溜めて batch 化する方式は、

- 帯域節約には効く
- 受信側 decode は軽くなりやすい
- ただしリアルタイム性は batch 待ちで悪化しやすい

というトレードオフです。

特に重要なのは次です。

- `通信量削減` は record 数が増えるほど効く
- `遅延悪化` は到着レートに比例して悪化する
- 均等到着 stream を後から溜めるより、同一 control tick の snapshot を最初からまとめて生成する方がはるかに健全

もし制御系で使うなら、推奨は

1. `command` は単発または小 batch
2. `telemetry` は batch 優先
3. flush 条件は `record_count` だけでなく `max_wait_ms` も併用

です。

## サイズ比較

### 1 件だけ送る場合

header / seq / CRC をちゃんと付けると、単発では劇的には減りません。

| ケース | 現状 ASCII | 推奨 binary |
| --- | ---: | ---: |
| Up 1 件 (`0x300,1234\r\n`) | 12 byte | 11 byte 前後 |
| Down 1 件 (`400,21.10\r\n`) | 11 byte | 11 byte 前後 |

このため、`単発 packet 化だけで満足しない` 方がよいです。

### まとめて送る場合

10 件 / 12 件を 1 packet にまとめると効果が大きいです。

前提:

- frame overhead: 8 byte
- 通常 record: `signal_code 1 byte + value 2 byte = 3 byte`

| ケース | 現状 ASCII | 推奨 binary |
| --- | ---: | ---: |
| Up 10 件 | `12 * 10 = 120` byte | `8 + 3 * 10 = 38` byte |
| Down 12 件 | `11 * 12 = 132` byte | `8 + 3 * 12 = 44` byte |

おおむね 65% 以上の削減が見込めます。

`float` / `double` 系でも、ASCII 桁数が長いほど有利です。

## チェック機能の推奨

### CRC

`PacketACv6` と同じ `CRC16-CCITT-FALSE` を推奨します。

理由:

- 既存実装を流用できる
- ASCII 行より強い破損検出ができる
- header + payload 全体を一括で検査できる

### sequence

`seq: u8` を持たせます。

これにより次ができます。

- 欠落 packet の検出
- 重複受信の検出
- ACK / NACK を付けた場合の対応付け

### ACK の要否

まずは `CRC + seq` のみで十分です。

ただし、制御系で「絶対に届いたことを知りたい」signal があるなら `flags.bit0 = ack_required` を設け、受信側が `seq` を返す ACK packet を返せるようにしてもよいです。

## 互換性と移行案

### 段階移行を推奨

1. ASCII `RoverUpGeneral` / `RoverDownGeneral` は残す
2. 並行して `RoverBinaryBatchV1` を追加する
3. `signal definition table` を固める
4. `float` / `double` 文字列信号を順次 `fixed-point` または `f32` へ移す
5. 十分に安定したら本番系を binary 優先にする

### デバッグ運用

ASCII は人間に優しいので、完全廃止よりも

- 本番 / 高速リンク: binary
- デバッグ / 手打ち確認: ASCII

の 2 系統を残す方が現実的です。

## 推奨事項まとめ

- 方向性としては十分可能
- 単発送信より `batch 化` を優先
- ID は `legacy_id` を文字列で送らず `signal_code` 化
- 値は `fixed-point` を第一候補
- `PacketACv6` に合わせて `header + seq + CRC16-CCITT-FALSE + little-endian`
- `float` / `double` は例外扱いにし、基本は scale 固定で整数化

## この案の実装イメージ

最低限の追加実装は次の単位になります。

- `OutputFormat` に新 format を追加
- binary frame encoder / decoder を追加
- `signal definition table` を定義
- `send` / `monitor` / `xbee-test` に binary rover frame の認識を追加

特に `send.rs` と `xbee_test.rs` は今の ASCII 前提の prefix 判定を持っているため、binary 化するなら `PacketACv6` と同様の `header + CRC` ベースの再同期へ寄せるのが自然です。
