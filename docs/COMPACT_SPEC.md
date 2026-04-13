# `compact` 仕様と HID レポート変換

## 概要

このプロジェクトの内部入力形式は、固定長 8 byte の `CompactReport` である。

```rust
pub type CompactReport = [u8; 8];
```

変換処理は `src/input/compact.rs` にあり、入力 HID レポートを次の流れで処理する。

```text
HID report -> hid10 に正規化 -> compact[0..7]
```

## 入力の正規化

`convert_input_report()` は入力長だけを見て、先頭 10 byte を共通表現 `hid10[0]..hid10[9]` として切り出す。

- `report.len() >= 64` の場合
  - 64 byte レポートとして扱い、`report[0..9]` を使う
- `report.len() >= 10` の場合
  - 10 byte レポートとして扱い、`report[0..9]` を使う
- `report.len() < 10` の場合
  - `ReportTooShort` を返す

この実装では `report_id` の値そのものは検証していない。

## `compact` のレイアウト

| byte | 内容 |
| ---: | --- |
| `0` | デジタル入力 1 |
| `1` | デジタル入力 2 |
| `2` | `lx` |
| `3` | `ly` |
| `4` | `rx` |
| `5` | `ry` |
| `6` | `l2` |
| `7` | `r2` |

### `compact[0]`

| bit | 内容 |
| ---: | --- |
| `0` | `up` |
| `1` | `right` |
| `2` | `down` |
| `3` | `left` |
| `4` | `square` |
| `5` | `cross` |
| `6` | `circle` |
| `7` | `triangle` |

### `compact[1]`

| bit | 内容 |
| ---: | --- |
| `0` | `l1` |
| `1` | `r1` |
| `2` | `share` |
| `3` | `options` |
| `4` | `l3` |
| `5` | `r3` |
| `6` | `ps` |
| `7` | `trackpad_click` |

## `hid10` から `compact` への変換

### 1. D-pad と face buttons

`hid10[5]` の下位 4 bit は D-pad 値、上位 4 bit は `square/cross/circle/triangle` である。

D-pad は次のように 4 方向ビットへ展開する。

| `hid10[5] & 0x0f` | `compact[0]` bit0-3 |
| ---: | --- |
| `0` | `0001` |
| `1` | `0011` |
| `2` | `0010` |
| `3` | `0110` |
| `4` | `0100` |
| `5` | `1100` |
| `6` | `1000` |
| `7` | `1001` |
| `8` | `0000` |

`0..8` 以外の値は `InvalidDpad` を返す。

face buttons はそのまま上位 4 bit をコピーする。

```text
compact[0] = dpad_bits | (hid10[5] & 0xf0)
```

### 2. ショルダー、システム、押し込み

`compact[1]` は `hid10[6]` と `hid10[7]` から組み立てる。

| `compact[1]` bit | 元のビット |
| ---: | --- |
| `0` | `hid10[6]` bit0 (`l1`) |
| `1` | `hid10[6]` bit1 (`r1`) |
| `2` | `hid10[6]` bit4 (`share`) |
| `3` | `hid10[6]` bit5 (`options`) |
| `4` | `hid10[6]` bit6 (`l3`) |
| `5` | `hid10[6]` bit7 (`r3`) |
| `6` | `hid10[7]` bit0 (`ps`) |
| `7` | `hid10[7]` bit1 (`trackpad_click`) |

実装上の式は次の通り。

```text
compact[1] = (hid10[6] & 0x03)
           | ((hid10[6] >> 2) & 0x3c)
           | ((hid10[7] & 0x03) << 6)
```

### 3. アナログ入力

アナログ値はコピーである。

```text
compact[2] = hid10[1]
compact[3] = hid10[2]
compact[4] = hid10[3]
compact[5] = hid10[4]
compact[6] = hid10[8]
compact[7] = hid10[9]
```

## 擬似コード

```text
hid10 = first 10 bytes of report

dpad = hid10[5] & 0x0f

compact[0] = expand_dpad(dpad) | (hid10[5] & 0xf0)
compact[1] = (hid10[6] & 0x03) | ((hid10[6] >> 2) & 0x3c) | ((hid10[7] & 0x03) << 6)
compact[2] = hid10[1]
compact[3] = hid10[2]
compact[4] = hid10[3]
compact[5] = hid10[4]
compact[6] = hid10[8]
compact[7] = hid10[9]
```

## 実装上の性質

- 10 byte 入力と 64 byte 入力で先頭 10 byte が同じなら、生成される `compact` も同じになる
- `usb[10]` 以降の情報は `compact` に影響しない
- `compact` は D-pad を 4 方向の同時押し表現へ展開して保持する

## 変換例

入力:

```text
hid10 = [0x11, 0xff, 0x80, 0x01, 0x7f, 0x07, 0x52, 0x03, 0xff, 0x40]
```

出力:

```text
compact = [0x09, 0xd6, 0xff, 0x80, 0x01, 0x7f, 0xff, 0x40]
```

## 根拠

- `src/input/compact.rs`
