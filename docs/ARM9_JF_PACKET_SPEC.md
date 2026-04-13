# `arm9` JF パケット最新版仕様

## 対象

この文書は、`arm9_ik` 系で使われている downlink `JF` パケットについて、
2026-04-14 JST 時点で **他ブランチも含めて有効な最新版**を特定し、
人間や AI エージェントがそのまま再実装できる粒度で整理した仕様書です。

出力先が `acs` リポジトリなので、`acs/docs/PACKET_ACV6_PACKET_AND_CAN.md` の対になる
`JF` 側の仕様書として読めるように構成しています。

## 結論

この調査時点の **最新版 JF パケット**は、固定長 16 byte の `JF v1` です。

- header: `b"JF"`
- wire format: little-endian `<2sBB5HH>`
- packet length: 16 byte
- field order: `header, seq, flags, encoders[5], crc16`
- CRC: `CRC-16/CCITT-FALSE`

重要なのは、最新版は単に「16 byte である」だけではないことです。
現行 `master` 系では次もセットで最新版仕様に含まれます。

- `J0` は単なる angle slot ではなく、**linear guide の signed-mm feedback** として運用する
- `flags` は `bit0=YAMAN_READY, bit1=READY, bit2=X_ALIGN_DONE, bit3=GLOBAL_YZ_DONE, bit4=LOCAL_YZ_DONE, bit5=HOME_YZ_DONE, bit6=HOME_X_DONE`
- serial 側は packet 境界が保証されないため、**header scan + CRC で再同期**する

## 調査結果の要約

### 調査した branch / ref

JF 関連ファイルを持つ現行 ref を追い、少なくとも次を確認しました。

- `origin/master` (`0aa5fe6`, 2026-03-28 22:41:18 +0900)
- `origin/release/v1.2.1` (`9ea1be2`, 2026-03-14 13:26:25 +0900)
- `origin/feat/keyboard-v2` (`3cdaf26`, 2026-03-13 19:02:05 +0900)
- `origin/feat/keyboard-v1` (`c1d60a8`, 2026-03-30 20:40:35 +0900)
- `origin/feat/keybaord-v1_seemless-ik` (`8aa6bc6`, 2026-03-30 14:02:17 +0900)
- `origin/feat/keyboard-auto` (`56574d7`, 2026-04-01 21:58:37 +0900)
- `origin/feat/keyboard-auto-coordinate-transform` (`86ebde2`, 2026-04-12 16:06:33 +0900)
- `origin/feat/keyboard-auto-rate-launch-args` (`aecb587`, 2026-04-01 02:12:17 +0900)
- `origin/feat/keyboard-auto-extra-rate-launch-args` (`f8191b3`, 2026-04-01 02:29:04 +0900)

### 版の結論

- **wire format 自体**は、現行 branch 群で `<2sBB5HH>` / 16 byte に収束しています
- ただし **意味づけ**は branch により差分があります
- `release/v1.2.1` と `feat/keyboard-v2` までは、
  - `bit0=READY, bit1=X_ALIGN_DONE, bit2=GLOBAL_YZ_DONE, bit3=LOCAL_YZ_DONE, bit4=HOME_YZ_DONE, bit5=HOME_X_DONE`
  - `J0` はまだ linear guide としては未採用
- `master` と 2026-03-26 以降の feature branch では、
  - `bit0=YAMAN_READY, bit1=READY, bit2..6=phase done`
  - `J0` を signed-mm linear feedback として扱う

つまり、**最新版仕様は `master` 系の 16 byte JF**であり、`release/v1.2.1` は
「同じ 16 byte だが意味が古い互換世代」とみなすのが正確です。

## ソースオブトゥルース

最新版仕様の根拠になる主な実装は次です。

- `arm9_ik/src/arm_ik_control/arm_ik_control/udp_joint_state_rx.py`
- `arm9_ik/src/arm_ik_control/config/udp_joint_state_rx.yaml`
- `arm9_ik/tools/dashboard_bridge.py`
- `arm9_ik/src/arm_ik_control/config/keyboard_auto_controller.yaml`
- `arm9_ik/src/arm_ik_control/arm_ik_control/keyboard_auto_feedback.py`
- `arm9_ik/src/arm_ik_control/arm_ik_control/moveit_ik_controller.py`
- `arm9_ik/tools/stm_enter_mock.py`
- `arm9_ik/tools/stm_less_check.py`
- `arm9_ik/docs/communication_data_formats.md`
- `arm9_ik/docs/keyboard_auto_workflow.md`

検証用テストとして次も有効です。

- `arm9_ik/src/arm_ik_control/test/test_udp_joint_state_rx_flags_freshness.py`
- `arm9_ik/src/arm_ik_control/test/test_keyboard_auto_controller_flag_edges.py`
- `arm9_ik/src/arm_ik_control/test/test_joint_state_feedback_utils.py`
- `arm9_ik/tools/test_stm_less_check.py`
- `arm9_ik/tools/prod_runtime_checks/test_stream_runtime.py`

## 全体フロー

現行 default topology では、`JF` は次の向きで流れます。

```text
STM32 -> XBee -> Mac dashboard_bridge -> UDP 5010 -> Ubuntu udp_joint_state_rx -> ROS topics
                                   \-> UDP 4102 telemetry mirror -> dashboard UI
```

packet 境界の扱いは経路で異なります。

- STM32 / XBee / serial 側:
  - **stream**
  - 読み出し chunk 境界は packet 境界と一致しない
- Mac -> Ubuntu UDP5010:
  - **1 datagram = 1 JF frame**
- telemetry UDP4102:
  - raw `JF` 16 byte をそのまま mirror する場合がある
- WebSocket:
  - binary ではなく、bridge が展開した JSON (`type="jf"`) を流す

## Wire Format

### 16 byte 固定レイアウト

`JF` の最新版 wire format は以下です。

```text
<2sBB5HH
```

| offset | size | 型 | field | 内容 |
| --- | ---: | --- | --- | --- |
| `0..1` | 2 | `[u8; 2]` | `header` | 常に `b"JF"` (`0x4A 0x46`) |
| `2` | 1 | `u8` | `seq` | 8 bit 連番。wrap 可 |
| `3` | 1 | `u8` | `flags` | STM32 側 status / keyboard-auto ACK 群 |
| `4..5` | 2 | `u16` | `encoders[0]` | `J0` raw word |
| `6..7` | 2 | `u16` | `encoders[1]` | `J1` raw word |
| `8..9` | 2 | `u16` | `encoders[2]` | `J2` raw word |
| `10..11` | 2 | `u16` | `encoders[3]` | `J3` raw word |
| `12..13` | 2 | `u16` | `encoders[4]` | `J4` raw word |
| `14..15` | 2 | `u16` | `crc16` | bytes `0..13` に対する CRC16-CCITT-FALSE |

注意:

- UI / bridge JSON に出てくる `proto: 1` は **wire 上の field ではありません**
- これは `udp_joint_state_rx` や `dashboard_bridge` が
  「現行 fixed 16-byte JF を internal に proto=1 とラベル付けしているだけ」
  の管理情報です

### エンディアン

- すべて little-endian
- `u16` も `crc16` も little-endian

### header

- ASCII 2 byte 固定
- 値は常に `b"JF"`

serial stream ではこの header を足がかりに再同期しますが、header だけでは不十分です。
**必ず CRC まで通して valid frame と判定**します。

### seq

- `0..255` の wrapping counter
- consumer 側では主に alive 確認用に使う
- 現行 `udp_joint_state_rx` は gap 補完や reorder はしない
- socket / serial buffer を drain して **最後の valid frame だけ採用**するため、
  seq の飛びは通常動作でも起こり得る

### flags

最新版の `flags` は次です。

| bit | 名前 | 意味 |
| ---: | --- | --- |
| `0` | `YAMAN_READY` | pre-READY wrist-90 handshake 完了 |
| `1` | `READY` | その文字 cycle を始められる STM 準備完了 |
| `2` | `X_ALIGN_DONE` | global X 完了 |
| `3` | `GLOBAL_YZ_DONE` | coarse global Y-Z 完了 |
| `4` | `LOCAL_YZ_DONE` | final local Y-Z 完了 |
| `5` | `HOME_YZ_DONE` | keyboard home の Y-Z 復帰完了 |
| `6` | `HOME_X_DONE` | keyboard home の X 復帰完了 |
| `7` | reserved | 現行未使用。`0` 推奨 |

重要なのは、`flags` は wire 上では単なる level bit 群であり、
**edge 判定や freshness 判定は consumer 側の上位ロジックで付く**ことです。

現行 consumer の扱いは次です。

- `bit0` (`YAMAN_READY`)
  - `AC control_byte bit2` の request を見て latch 用の待ち状態を arm する
  - **low -> high を見たら latched true**
- `bit1` (`READY`)
  - fresh な high を受理
  - 現行 default では edge 必須ではない
- `bit2..6`
  - phase 専用 done bit
  - **low を一度見てから high になる edge** を完了として扱う
  - stale bit の誤検出を防ぐため、`jf_done_requires_edge=true`

## `encoders[5]` の意味

wire 上は 5 本とも単なる `u16` ですが、最新版運用では slot ごとの意味が異なります。

### 現行 deployed mapping

`bs_moveit_teleop.launch.py` は `udp_joint_state_rx.yaml` を読み込むため、
実際の現行運用では次の意味になります。

| slot | current meaning | decoder behavior | ROS joint |
| ---: | --- | --- | --- |
| `J0` | linear guide raw word | `u16` を `i16` として再解釈し、mm -> m 変換 | `base_link_to_horizontal` |
| `J1` | encoder14 angle | `0..16383 -> 0..2pi -> [-pi, pi]`, さらに unwrap | `base_linke_to_dodai` |
| `J2` | encoder14 angle | `0..16383 -> 0..2pi -> [-pi, pi]` | `dodai_to_link1` |
| `J3` | encoder14 angle | `0..16383 -> 0..2pi -> [-pi, pi]` | `link1_to_link2` |
| `J4` | encoder14 angle | `0..16383 -> 0..2pi -> [-pi, pi]` | `link2_to_link3` |

### `J0` の signed-mm 解釈

最新版で最も誤解しやすい点が `J0` です。

`J0` は最新 `master` 系では angle count ではなく、**linear feedback の signed word** です。
`udp_joint_state_rx` は次の手順で decode します。

1. `raw_u16` を 16 bit 2 の補数付き `i16` として再解釈
2. `signed_counts * mm_per_count / 1000.0` で meter に変換

現行 YAML では `linear_feedback_mm_per_count: 1.0` なので、

- `0x000C` -> `+12 mm` -> `0.012 m`
- `0xFFE7` -> `-25 mm` -> `-0.025 m`

になります。

注意:

- `udp_joint_state_rx.py` の bare default は `linear_feedback_mm_per_count=0.1` ですが、
  **現行 launch で読み込まれる YAML は `1.0`** です
- よって「最新版運用仕様」を再現したいなら、node のコード default ではなく
  **YAML 込みの runtime 値**を採用してください

### `J1..J4` の angle decode

`J1..J4` は 14 bit 相当の encoder count として扱います。

```text
theta = (count % 16384) * 2pi / 16384
theta_wrapped = wrap_to_pi(theta)
```

現行 default では `J1` のみ continuous joint として unwrap します。

### 旧世代との差

`release/v1.2.1` までは `J0` を linear guide としてまだ採用しておらず、
`udp_joint_state_rx.yaml` にも
「`J0` は prismatic joint と意味が合わないので unmapped のまま」
という扱いが残っています。

したがって、**同じ `<2sBB5HH>` でも `J0` の意味は世代で違う**と考えてください。

## CRC16

`JF` は `CRC-16/CCITT-FALSE` を使います。

- polynomial: `0x1021`
- init: `0xFFFF`
- refin: `false`
- refout: `false`
- xorout: `0x0000`

CRC の対象は frame 先頭から byte `13` まで、つまり
`header + seq + flags + encoders[5]` の 14 byte です。

末尾の `crc16` には little-endian で格納します。

既知ベクタ:

```text
crc16_ccitt_false("123456789") = 0x29B1
```

## 再実装ルール

### binary encoder

最小限の encoder は次の手順です。

1. `b"JF"` を入れる
2. `seq` を 1 byte で入れる
3. `flags` を 1 byte で入れる
4. `encoders[5]` を little-endian `u16` で順に入れる
5. 先頭 14 byte に CRC16-CCITT-FALSE をかける
6. CRC を little-endian `u16` で末尾に付ける

### binary decoder

UDP frame を decode するなら次で十分です。

1. `len == 16` を確認
2. `frame[0..2] == b"JF"` を確認
3. `crc16(frame[0..14]) == u16::from_le_bytes(frame[14..16])` を確認
4. `seq`, `flags`, `encoders[5]` を取り出す

### serial stream decoder

serial / XBee 側では packet 境界が無いので、現行 bridge は次の方針です。

1. buffer から `JF` header を探す
2. `JF` 以降 16 byte 揃うまで待つ
3. CRC が通れば 1 frame として確定して buffer から消費
4. CRC が通らなければ **1 byte だけ捨てて再探索**

この「bad CRC 時に 1 byte だけ落として resync」が重要です。
chunk 境界頼みや 16 byte 固定読みだけでは壊れた stream から復帰できません。

## WebSocket / telemetry mirror で見える派生データ

`dashboard_bridge` は raw `JF` をそのまま browser へ送らず、次の JSON に展開します。

```json
{
  "type": "jf",
  "proto": 1,
  "seq": 1,
  "flags": 0,
  "crc_ok": true,
  "encoders": [0, 0, 3281, 6397, 980],
  "angles_rad": [0.0, 0.0, 1.2583, 2.4531, 0.3760],
  "raw_hex": "4a46010000000000d10cfd18d4030486"
}
```

ここで注意点があります。

- `encoders` は raw wire 値そのもの
- `raw_hex` も raw wire 値そのもの
- `angles_rad` は bridge が `encoder14_to_rad()` を **5 slot 全部に機械的に適用した派生値**

したがって、最新版仕様では
**`angles_rad[0]` は `J0 signed-mm` の物理意味と一致しません**。

再実装時に信頼すべき順序は次です。

1. raw wire bytes
2. `encoders[5]`
3. 最新 YAML を反映した decoder
4. `angles_rad` は補助表示

## ROS / runtime surfaces

現行 launch (`bs_moveit_teleop.launch.py`) では `udp_joint_state_rx.yaml` が読み込まれ、
`JF` は次の ROS surface に変換されます。

| surface | 型 | 意味 |
| --- | --- | --- |
| `/arm_ik/joint_states_feedback` | `sensor_msgs/JointState` | latest valid JF を joint へ decode した feedback |
| `/arm_ik/jf_rx_seq` | `std_msgs/UInt8` | last seen seq |
| `/arm_ik/jf_rx_flags` | `std_msgs/UInt8` | last seen raw flags |
| `/arm_ik/jf_rx_proto` | `std_msgs/UInt8` | internal proto id。現行は `1` |
| `/arm_ik/jf_rx_crc_ok` | `std_msgs/UInt8` | last valid packet の CRC 結果。現行 valid path では `1` |
| `/arm_ik/jf_rx_alive` | `std_msgs/UInt8` | `alive_timeout_sec` 以内なら `1` |
| `/arm_ik/jf_rx_rate_hz` | `std_msgs/Float32` | receive rate |
| `/arm_ik/jf_rx_age_ms` | `std_msgs/Float32` | latest packet の age |

現行 receiver の挙動で重要なのは次です。

- `/arm_ik/jf_rx_flags` は **生 JF 受信時にだけ即 publish** される
- status timer は `flags` freshness を延命しない
- `/arm_ik/joint_states_feedback` は downlink が alive の間だけ publish される
- これにより、feedback が止まると上位 mux / controller が stale を検出できる

## 現行 keyboard-auto との関係

最新版 `JF` は keyboard-auto の ACK channel を兼ねています。

### STM が返すべき最小 contract

1 文字 cycle を通す最小 contract は次です。

- `bit0`: `AC control_byte bit2` の request に対する `YAMAN_READY`
- `bit1`: cycle 開始可の `READY`
- `bit2`: global X 完了
- `bit3`: global Y-Z 完了
- `bit4`: local Y-Z 完了
- `bit5`: keyboard_home Y-Z 完了
- `bit6`: keyboard_home X 完了

### 上位 consumer の待ち方

現行 Ubuntu 側は概ね次の順で待ちます。

1. `YAMAN_READY`
2. `READY` と miniPC start ACK の両方
3. `X_ALIGN_DONE`
4. `GLOBAL_YZ_DONE`
5. `LOCAL_YZ_DONE`
6. `HOME_YZ_DONE`
7. `HOME_X_DONE`

### freshness

`flags` を見たというだけでは不十分で、現行 consumer は freshness も見ます。

- `kbd_yaman_flag_fresh_sec = 0.5`
- `stm_ready_flag_fresh_sec = 0.5`
- `jf_done_flag_fresh_sec = 0.5`

つまり、**0.5 秒以上古い `JF.flags` は current ACK として扱わない**のが現行 default です。

## 実装例

### Rust での最小 parser

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JfFrame {
    pub seq: u8,
    pub flags: u8,
    pub encoders: [u16; 5],
    pub crc16: u16,
}

pub fn parse_jf(frame: &[u8]) -> Option<JfFrame> {
    if frame.len() != 16 {
        return None;
    }
    if &frame[0..2] != b"JF" {
        return None;
    }

    let crc_rx = u16::from_le_bytes([frame[14], frame[15]]);
    let crc_calc = crc16_ccitt_false(&frame[0..14]);
    if crc_rx != crc_calc {
        return None;
    }

    Some(JfFrame {
        seq: frame[2],
        flags: frame[3],
        encoders: [
            u16::from_le_bytes([frame[4], frame[5]]),
            u16::from_le_bytes([frame[6], frame[7]]),
            u16::from_le_bytes([frame[8], frame[9]]),
            u16::from_le_bytes([frame[10], frame[11]]),
            u16::from_le_bytes([frame[12], frame[13]]),
        ],
        crc16: crc_rx,
    })
}
```

### Rust での最小 encoder

```rust
pub fn build_jf(seq: u8, flags: u8, encoders: [u16; 5]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0] = b'J';
    out[1] = b'F';
    out[2] = seq;
    out[3] = flags;

    for (i, value) in encoders.iter().enumerate() {
        let bytes = value.to_le_bytes();
        let base = 4 + i * 2;
        out[base] = bytes[0];
        out[base + 1] = bytes[1];
    }

    let crc = crc16_ccitt_false(&out[0..14]).to_le_bytes();
    out[14] = crc[0];
    out[15] = crc[1];
    out
}
```

## mock / synthetic sender の違い

再現用の sender は 2 系統あり、用途が違います。

### `tools/stm_enter_mock.py` / `tools/stm_less_check.py`

- 実機 STM の代役
- `flags` の handshake / done bit を返す
- `J0..J4` を全部埋められる
- `default home` 例もこの系統が source

### `udp_joint_state_telemetry_tx`

- no-hardware 時に `/joint_states` から synthetic JF を作る Ubuntu 側 debug sender
- 現行実装では `encoders = (0, 0, J2, J3, J4)` を送る
- つまり **J0/J1 は 0 固定**で、keyboard-auto の本格的な STM 代役にはならない
- 主用途は UI / RViz の pose 更新確認

## 既知のサンプル frame

### default fake home frame

`tools/stm_enter_mock.py` / `tools/stm_less_check.py` の default home encoder から作る
代表 frame は次です。

- `seq = 0x01`
- `flags = 0x00`
- `encoders = [0, 0, 3281, 6397, 980]`
- `crc16 = 0x8604`

hex:

```text
4a46010000000000d10cfd18d4030486
```

16 byte に区切ると:

```text
4A 46 01 00 00 00 00 00 D1 0C FD 18 D4 03 04 86
```

### signed-mm を含む例

`J0=+12 mm`, `J1=0x3FFE`, `J2=0`, `J3=0x2000`, `J4=0x1234`,
`seq=0x34`, `flags=0x4F` の frame 例:

```text
4A 46 34 4F 0C 00 FE 3F 00 00 00 20 34 12 67 4B
```

この例では CRC は `0x4B67` です。

## 履歴

最新版に至る流れだけを残します。

### 1. legacy `JS` float packet

`2026-01-17` より前は `JF` ではなく、legacy `JS` float feedback が主でした。

- format: `<2sBBfffff>`
- fixed binary だが current system では obsolete

### 2. 2026-01-17: `JF` 導入

commit:

- `2c24374` (`2026-01-17 07:31:21 +0000`) `Add JF encoder14 angle-only feedback for RViz`
- `4213f98` (`2026-01-17 08:23:44 +0000`) `Simplify JF feedback decode (no offsets/directions)`

この世代の `JF` は:

- format: `<2sBB5H>`
- length: 14 byte
- CRC なし
- `J0..J4` 全部を angle count 扱い

### 3. 2026-02-23: `J2` 一時導入

commit:

- `4a2afc9` (`2026-02-23 23:24:20 +0900`) `feat(protocol): add AC v6 CRC + J2 feedback frames`

この時点では一時的に:

- `JF` は 14 byte のまま
- 別 proto として `J2` (`<2sBB2H5HH>`) が存在
- `x_done_count` / `yz_done_count` と CRC を持つ

しかしこの `J2` は現行 branch 群には残っていません。
**最新版では obsolete** です。

### 4. 2026-03-10: 現行 16 byte `JF` へ収束

commit:

- `cd4140b` (`2026-03-10 02:49:22 +0900`) `feat(keyboard-auto): split JF done flags by phase and document workflow`

ここで現行の

- `<2sBB5HH>`
- 16 byte fixed length
- CRC16 付き

へ整理されます。

### 5. 2026-03-17: `J0` を linear guide に採用

commit:

- `01e2c12` (`2026-03-17 20:46:59 +0900`) `feat(feedback): map JF J0 into linear guide pose`

ここで `J0` は「未使用に近い angle slot」から、
**signed-mm linear feedback** という現在の意味に変わります。

### 6. 2026-03-22: flags freshness を event-driven 化

commit:

- `1669f99` (`2026-03-22 20:49:29 +0900`) `fix(keyboard-auto): keep JF flag freshness event-driven`

ここで `JF.flags` は status timer で再 publish せず、
**生 frame 受信時だけ freshness を更新する**運用が固まります。

### 7. 2026-03-26: current flag semantics

commit:

- `d504953` (`2026-03-26 19:33:55 +0900`) `feat(keyboard-auto): update yaman workflow and controls`

ここで現行の

- `bit0=YAMAN_READY`
- `bit1=READY`
- `bit2..6=phase done`

という最新版意味づけになります。

## 互換性の線引き

最新版実装として互換を名乗るなら、最低でも次を満たすべきです。

1. raw wire format が `<2sBB5HH>` / 16 byte / little-endian である
2. CRC16-CCITT-FALSE が一致する
3. `bit0..6` の意味が current mapping である
4. `J0` を signed-mm linear feedback として解釈できる
5. serial stream では header scan + CRC resync を行う

反対に、次は「旧世代互換」であって最新版互換ではありません。

- `<2sBB5H>` の 14 byte `JF`
- `J2` proto
- `bit0=READY, bit1=X_ALIGN_DONE, ...` の旧 flag layout
- `J0` を angle count のまま扱う decoder

## 根拠

- `arm9_ik/src/arm_ik_control/arm_ik_control/udp_joint_state_rx.py`
- `arm9_ik/src/arm_ik_control/config/udp_joint_state_rx.yaml`
- `arm9_ik/tools/dashboard_bridge.py`
- `arm9_ik/src/arm_ik_control/config/keyboard_auto_controller.yaml`
- `arm9_ik/src/arm_ik_control/arm_ik_control/keyboard_auto_feedback.py`
- `arm9_ik/src/arm_ik_control/arm_ik_control/moveit_ik_controller.py`
- `arm9_ik/tools/stm_enter_mock.py`
- `arm9_ik/tools/stm_less_check.py`
- `arm9_ik/src/arm_ik_control/test/test_udp_joint_state_rx_flags_freshness.py`
- `arm9_ik/src/arm_ik_control/test/test_keyboard_auto_controller_flag_edges.py`
- `arm9_ik/src/arm_ik_control/test/test_joint_state_feedback_utils.py`
