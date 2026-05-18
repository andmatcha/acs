# `PacketUFv2` パケット仕様

`PacketUFv2` は STM32 から frontend へ流れる USB memory text feedback 用の downlink packet です。
USB memory から読み取った byte 列を座標へ解釈せず、32 byte chunk としてそのまま返します。

## Wire Format

| field | type | byte offset | notes |
| --- | --- | ---: | --- |
| header | `2s` | 0 | `b"UF"` |
| seq | `uint8` | 2 | STM sequence |
| flags | `uint8` | 3 | bit layout は下表 |
| chunk_index | `uint8` | 4 | 0 origin の chunk 番号 |
| payload_len | `uint8` | 5 | 有効 payload byte 数、`0..32` |
| payload | `uint8[32]` | 6 | text byte 列。`payload_len` 以降は padding |
| crc16 | `uint16` | 38 | CRC-16/CCITT-FALSE over first 38 bytes, little-endian |

Packet length は 40 byte です。

## `flags`

| bit | meaning |
| ---: | --- |
| 0 | `valid` |
| 1 | `usb_present` |
| 2 | `read_busy` |
| 3 | `read_error` |
| 4 | `end` |
| 5-7 | reserved, send `0` |

## `acs`

monitor 系の受信 format として `packetufv2` / `PacketUFv2` を指定できます。
CRC OK の 40 byte UF v2 packet だけを処理し、旧 14 byte UF packet は認識対象から外しています。
`valid` chunk は `chunk_index` の連続性を確認しながら復元し、`end` で完了した text を表示します。
`read_busy` / `read_error` / timeout は status 行として表示します。

```bash
acs control --monitor /dev/ttyUSB1@921600,hex+packet,packetufv2
acs io -i /dev/ttyUSB1@921600,hex+packet,packetufv2
```
