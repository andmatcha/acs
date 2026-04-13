# DUALSHOCK 4 HIDレポート仕様

## 対象

このプロジェクトでは、DUALSHOCK 4 の入力を次の 2 系統で扱う。

- 10 byte の短い HID レポート
- 64 byte の USB HID レポート

`src/input/compact.rs` では、どちらの入力でも先頭 10 byte を共通入力として扱う。以降ではこの共通部分を `hid10[0]..hid10[9]` と呼ぶ。

## 10 byte レポート

10 byte レポートのバイト割り当ては次の通り。

| byte | フィールド | 内容 |
| ---: | --- | --- |
| `0` | `report_id` | レポート ID |
| `1` | `lx` | 左スティック X |
| `2` | `ly` | 左スティック Y |
| `3` | `rx` | 右スティック X |
| `4` | `ry` | 右スティック Y |
| `5` | `buttons[0]` | D-pad と face buttons |
| `6` | `buttons[1]` | L1/R1/Share/Options/L3/R3 |
| `7` | `buttons[2]` | PS、trackpad click ほか |
| `8` | `l2` | L2 アナログ値 |
| `9` | `r2` | R2 アナログ値 |

## ボタン対応

### `buttons[0]`

`buttons[0]` の下位 4 bit は D-pad、上位 4 bit は face buttons である。

| bit | 内容 |
| ---: | --- |
| `0..3` | D-pad 値 |
| `4` | `square` |
| `5` | `cross` |
| `6` | `circle` |
| `7` | `triangle` |

D-pad 値の対応は次の通り。

| 値 | 方向 |
| ---: | --- |
| `0` | Up |
| `1` | Up + Right |
| `2` | Right |
| `3` | Right + Down |
| `4` | Down |
| `5` | Down + Left |
| `6` | Left |
| `7` | Left + Up |
| `8` | Neutral |

### `buttons[1]`

| bit | 内容 |
| ---: | --- |
| `0` | `l1` |
| `1` | `r1` |
| `4` | `share` |
| `5` | `options` |
| `6` | `l3` |
| `7` | `r3` |

bit `2` と bit `3` は、このプロジェクトでは使っていない。

### `buttons[2]`

| bit | 内容 |
| ---: | --- |
| `0` | `ps` |
| `1` | `trackpad_click` |

bit `2` 以降は、このプロジェクトでは使っていない。

## 64 byte USB レポート

64 byte USB レポート全体のレイアウトは次の通り。

| オフセット | 長さ | フィールド | 内容 |
| --- | ---: | --- | --- |
| `0` | 1 | `report_id` | 入力レポート ID |
| `1` | 1 | `x` | 左スティック X (`lx`) |
| `2` | 1 | `y` | 左スティック Y (`ly`) |
| `3` | 1 | `rx` | 右スティック X |
| `4` | 1 | `ry` | 右スティック Y |
| `5` | 1 | `buttons[0]` | D-pad と face buttons |
| `6` | 1 | `buttons[1]` | L1/R1/Share/Options/L3/R3 |
| `7` | 1 | `buttons[2]` | PS、trackpad click ほか |
| `8` | 1 | `z` | L2 アナログ値 |
| `9` | 1 | `rz` | R2 アナログ値 |
| `10..11` | 2 | `sensor_timestamp` | センサー時刻 |
| `12` | 1 | `sensor_temperature` | センサー温度 |
| `13..18` | 6 | `gyro[3]` | ジャイロ 3 軸 |
| `19..24` | 6 | `accel[3]` | 加速度 3 軸 |
| `25..29` | 5 | `reserved2` | 予約領域 |
| `30..31` | 2 | `status` | バッテリー状態、接続状態など |
| `32` | 1 | `reserved3` | 予約領域 |
| `33` | 1 | `num_touch_reports` | タッチレポート数 |
| `34..42` | 9 | `touch_reports[0]` | タッチレポート 0 |
| `43..51` | 9 | `touch_reports[1]` | タッチレポート 1 |
| `52..60` | 9 | `touch_reports[2]` | タッチレポート 2 |
| `61..63` | 3 | `reserved` | 予約領域 |

このプロジェクトで `compact` 変換に使うのは `usb[0]..usb[9]` のみである。

## 10 byte と 64 byte の対応

USB レポートの先頭 10 byte は、10 byte レポートと同じ意味の入力に対応している。

| 共通表現 | 10 byte レポート | 64 byte USB レポート | 内容 |
| --- | --- | --- | --- |
| `hid10[0]` | `report[0]` | `usb[0]` | `report_id` |
| `hid10[1]` | `report[1]` | `usb[1]` | `lx` |
| `hid10[2]` | `report[2]` | `usb[2]` | `ly` |
| `hid10[3]` | `report[3]` | `usb[3]` | `rx` |
| `hid10[4]` | `report[4]` | `usb[4]` | `ry` |
| `hid10[5]` | `report[5]` | `usb[5]` | D-pad と face buttons |
| `hid10[6]` | `report[6]` | `usb[6]` | L1/R1/Share/Options/L3/R3 |
| `hid10[7]` | `report[7]` | `usb[7]` | PS、trackpad click ほか |
| `hid10[8]` | `report[8]` | `usb[8]` | `l2` |
| `hid10[9]` | `report[9]` | `usb[9]` | `r2` |

## このプロジェクトでの扱い

- `src/input/compact.rs` は 10 byte 以上の入力に対して先頭 10 byte を読む
- 64 byte 以上なら USB レポートとして扱い、同じく先頭 10 byte を使う
- `usb[10]` 以降のセンサー、状態、タッチ情報は `compact` 変換では使わない

## 根拠

- `src/input/compact.rs`
- Linux kernel `drivers/hid/hid-playstation.c`
  - `struct dualshock4_input_report_common`
  - `struct dualshock4_input_report_usb`
