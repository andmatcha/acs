# arm9 コントローラー入力と AC v6 / CAN 対応

`arm9` Manual モードで、DS4 の入力がどの AC v6 field と `kodenchan` direct CAN の ID / Data byte に載るかを一覧化したメモです。

電流値の表にあるデフォルト値と入力後の値は AC v6 の raw `current` 値です。`kodenchan` 側では `motor_command = (current - 255) * 64` に変換され、CAN data へ high byte first で載ります。raw `255` は CAN command `0` です。

## 電流値として載る入力

| コントローラー入力 | AC v6 field | `kodenchan` CAN | デフォルト raw | 入力後 raw |
| --- | --- | --- | ---: | --- |
| `R2 / L2` | `current[0]` | `0x200 Data[0..1]` | `255` | Normal: `155 / 355`、Power: `100 / 400`、Sensitive: `180 / 330` |
| `R1 / L1` | `current[1]` | `0x200 Data[2..3]` | `255` | Normal: `315 / 205`、Power: `511 / 1`、Sensitive: `295 / 225` |
| 右スティック Y | `current[2]` | `0x200 Data[4..5]` | `255` | 右スティック下/上。Normal: `230 / 280`、Power: `170 / 340`、Sensitive: `230 / 260` |
| 左スティック Y | `current[3]` | `0x200 Data[6..7]` | `255` | 左スティック下/上。Normal: `225 / 275`、Power: `175 / 325`、Sensitive: `215 / 280` |
| `triangle / cross` | `current[4]` | `0x1FF Data[0..1]` | `255` | Normal: `210 / 400`、Power: `160 / 450`、Sensitive: `225 / 290` |
| `circle / square` | `current[5]` | `0x1FF Data[2..3]` | `255` | Normal: `190 / 310`、Power: `80 / 430`、Sensitive: `210 / 300` |
| `D-pad Right / Left` | `current[6]` | `0x1FF Data[4..5]` | `255` | Normal: `155 / 285`、Power: `105 / 335`、Sensitive: `240 / 270` |

## ビットとして載る入力

| コントローラー入力 | `control_byte` | `kodenchan` CAN | デフォルト値 | 入力後の値 |
| --- | --- | --- | ---: | ---: |
| 未使用 | bit0 `KBD_PP` | `0x208 Data[0]` | `0` | `0` |
| 未使用 | bit1 `KBD_EN` | `0x208 Data[1]` | `0` | `0` |
| 未使用 | bit2 `KBD_YAMAN` | `0x208 Data[2]` | `0` | `0` |
| `D-pad Up` | bit3 `NYOKKI_PUSH` | `0x208 Data[3]` | `0` | `1` |
| `D-pad Down` | bit4 `NYOKKI_PULL` | `0x208 Data[4]` | `0` | `1` |
| `r3` | bit5 `INIT` | `0x208 Data[5]` | `0` | `1` |
| `l3` | bit6 `HOME` | `0x208 Data[6]` | `0` | `1` |
| 未使用 | bit7 `KBD_START` | `0x208 Data[7]` | `0` | `0` |

## 補足

- `share` は CAN に直接は載らず、`Normal -> Power -> Sensitive` のプロファイル切り替えに使う
- `options` は CAN に直接は載らず、`enable` のトグルに使う
- `enable == false` のとき、電流値として載る項目はすべて `255` に戻る

## 根拠

- [ARM9_ACV6_PACKET_AND_CAN.md](/Users/jinaoyagi/workspace/personal/ds4map/docs/ARM9_ACV6_PACKET_AND_CAN.md)
- [encoder.rs](/Users/jinaoyagi/workspace/personal/ds4map/src/output/formats/arm9/encoder.rs)
