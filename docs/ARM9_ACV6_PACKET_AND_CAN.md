# `arm9` AC v6 パケット仕様と CAN 変換

## 対象

この文書は、現行の `src/output/formats/arm9/encoder.rs` が生成する Manual 向け `AC v6` パケットと、その後段で使う CAN 変換を整理したものです。

このリポジトリが直接生成するのは `AC v6` パケットまでであり、CAN 変換の節は Manual モードで downstream 側がどのフィールドを CAN に載せるかをまとめています。

## 全体フロー

```text
DS4 HID -> compact -> arm9 ManualPacketEncoder -> AC v6 packet -> CAN
```

現行実装の `arm9` 出力は Manual モード専用で、生成されるパケット長は 39 byte である。

## `compact` から Manual 制御状態への対応

### enable とプロファイル

- `options` の立ち上がりで `enable` をトグルする
- `share` の立ち上がりでプロファイルを切り替える
  - `Normal -> Power -> Sensitive -> Normal`
- `enable == false` のときは `current[0..6]` をすべて `255` に戻す

### 閾値

| 入力 | 判定条件 |
| --- | --- |
| `l2` | `compact[6] >= 205` |
| `r2` | `compact[7] >= 205` |
| 左スティック上 | `compact[3] <= 25` |
| 左スティック下 | `compact[3] >= 230` |
| 右スティック上 | `compact[5] <= 25` |
| 右スティック下 | `compact[5] >= 230` |

### `current[0..6]` の対応

| `AC v6` field | 入力 | 中立時 | アクティブ時の値 |
| --- | --- | ---: | --- |
| `current[0]` | `R2 / L2` | `255` | BaseHorizon |
| `current[1]` | `R1 / L1` | `255` | BaseRoll |
| `current[2]` | 右スティック Y | `255` | Pitch1 |
| `current[3]` | 左スティック Y | `255` | Pitch2 |
| `current[4]` | `triangle / cross` | `255` | Pitch3 |
| `current[5]` | `circle / square` | `255` | Roll |
| `current[6]` | `D-pad Right / Left` | `255` | Gripper |

各チャンネルの値はプロファイルごとに次の通り。

| field | 正方向 / 負方向の入力 | Normal | Power | Sensitive |
| --- | --- | --- | --- | --- |
| `current[0]` | `R2 / L2` | `155 / 355` | `100 / 400` | `180 / 330` |
| `current[1]` | `R1 / L1` | `315 / 205` | `511 / 1` | `295 / 225` |
| `current[2]` | 右スティック下 / 上 | `230 / 280` | `170 / 340` | `230 / 260` |
| `current[3]` | 左スティック下 / 上 | `225 / 275` | `175 / 325` | `215 / 280` |
| `current[4]` | `triangle / cross` | `210 / 400` | `160 / 450` | `225 / 290` |
| `current[5]` | `circle / square` | `190 / 310` | `80 / 430` | `210 / 300` |
| `current[6]` | `Right / Left` | `155 / 285` | `105 / 335` | `240 / 270` |

### `control_byte`

`control_byte` は次のボタンから生成する。

| bit | 名前 | 入力 |
| ---: | --- | --- |
| `0` | `KBD_PP` | Manual encoder では未使用、`0` 固定 |
| `1` | `KBD_EN` | Manual encoder では未使用、`0` 固定 |
| `2` | `KBD_YAMAN` | Manual encoder では未使用、`0` 固定 |
| `3` | `NYOKKI_PUSH` | D-pad Up |
| `4` | `NYOKKI_PULL` | D-pad Down |
| `5` | `INIT` | `r3` |
| `6` | `HOME` | `l3` |
| `7` | `KBD_START` | Manual encoder では未使用、`0` 固定 |

## AC v6 パケット構造

パケットは 39 byte 固定長で、byte `0..36` が payload、byte `37..38` が CRC16 である。

| offset | size | 型 | フィールド | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `header` | 常に `b"AC"` |
| `2` | 1 | `u8` | `seq` | 送信ごとにインクリメント、`wrapping_add(1)` |
| `3` | 1 | `u8` | `flags` | Manual モードと enable |
| `4..17` | 14 | `u16[7]` | `current` | `current[0]..current[6]` |
| `18..23` | 6 | `u16[3]` | `angle` | 現行 Manual 実装ではすべて `0` |
| `24..29` | 6 | `i16[3]` | `vel` | 現行 Manual 実装ではすべて `0` |
| `30` | 1 | `u8` | `control_byte` | 共有 control byte |
| `31..32` | 2 | `i16` | `base_target_mm_j0` | 現行 Manual 実装では `0` |
| `33..34` | 2 | `u16` | `auto_flags` | 現行 Manual 実装では `0` |
| `35..36` | 2 | `u16` | `fault_code` | 現行 Manual 実装では `0` |
| `37..38` | 2 | `u16` | `crc16` | bytes `0..36` に対する CRC16-CCITT-FALSE |

### `flags`

`flags` の使い方は次の通り。

| bit | 内容 |
| ---: | --- |
| `0` | enable |
| `4..5` | control mode。現行実装では常に `1` (`Manual`) |

そのほかの bit は、この実装では 0 のままである。

### エンディアン

- `u16` / `i16` は little-endian で格納する
- CRC も little-endian で末尾へ付与する

### CRC16

CRC は `CRC16-CCITT-FALSE` で計算する。

- 初期値: `0xffff`
- poly: `0x1021`
- xorout: `0x0000`

## `kodenchan` direct CAN 変換

`kodenchan` は AC v6 の Manual packet を直接受け、旧 uplink CAN `0x200` / `0x201` ではなく motor CAN `0x200` / `0x1FF` / `0x208` を送る。

`current[0..6]` は次の式で `int16` の motor command になり、CAN data へ high byte first で格納される。

```text
motor_command = (current - 255) * 64
```

### CAN ID `0x200`

| CAN byte | 内容 |
| ---: | --- |
| `0` | motor0 MSB, from `current[0]` |
| `1` | motor0 LSB |
| `2` | motor1 MSB, from `current[1]` |
| `3` | motor1 LSB |
| `4` | motor2 MSB, from `current[2]` |
| `5` | motor2 LSB |
| `6` | motor3 MSB, from `current[3]` |
| `7` | motor3 LSB |

### CAN ID `0x1FF`

| CAN byte | 内容 |
| ---: | --- |
| `0` | motor4 MSB, from `current[4]` |
| `1` | motor4 LSB |
| `2` | motor5 MSB, from `current[5]` |
| `3` | motor5 LSB |
| `4` | motor6 MSB, from `current[6]` |
| `5` | motor6 LSB |
| `6` | `0x00` |
| `7` | `0x00` |

### CAN ID `0x208`

| CAN byte | 内容 |
| ---: | --- |
| `0` | `control_byte.bit0` = `KBD_PP` |
| `1` | `control_byte.bit1` = `KBD_EN` |
| `2` | `control_byte.bit2` = `KBD_YAMAN` |
| `3` | `control_byte.bit3` = `NYOKKI_PUSH` |
| `4` | `control_byte.bit4` = `NYOKKI_PULL` |
| `5` | `control_byte.bit5` = `INIT` |
| `6` | `control_byte.bit6` = `HOME` |
| `7` | `control_byte.bit7` = `KBD_START` |

### 入力から見た CAN 対応

| 入力 | `AC v6` field | CAN |
| --- | --- | --- |
| `R2 / L2` | `current[0]` | `0x200 Data[0..1]` |
| `R1 / L1` | `current[1]` | `0x200 Data[2..3]` |
| 右スティック Y | `current[2]` | `0x200 Data[4..5]` |
| 左スティック Y | `current[3]` | `0x200 Data[6..7]` |
| `triangle / cross` | `current[4]` | `0x1FF Data[0..1]` |
| `circle / square` | `current[5]` | `0x1FF Data[2..3]` |
| `D-pad Right / Left` | `current[6]` | `0x1FF Data[4..5]` |
| D-pad Up | `control_byte.bit3` | `0x208 Data[3]` |
| D-pad Down | `control_byte.bit4` | `0x208 Data[4]` |
| `r3` | `control_byte.bit5` | `0x208 Data[5]` |
| `l3` | `control_byte.bit6` | `0x208 Data[6]` |

## CAN に載らない項目

現行の Manual CAN 変換では、次の項目は使わない。

- `angle[0..2]`
- `vel[0..2]`
- `base_target_mm_j0`
- `auto_flags`
- `fault_code`
- `crc16`

## 根拠

- `src/output/formats/arm9/encoder.rs`
