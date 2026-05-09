# `PacketUFv1` パケット仕様

`PacketUFv1` は STM32 から frontend へ流れる USB memory latitude/longitude feedback 用の downlink packet です。
仕様は `kodenchan/docs/docs/communication_data_formats.md` の `UF` packet に合わせています。

## Wire Format

| field | type | byte offset | notes |
| --- | --- | ---: | --- |
| header | `2s` | 0 | `b"UF"` |
| seq | `uint8` | 2 | STM sequence |
| flags | `uint8` | 3 | bit layout は下表 |
| lat_e7 | `int32` | 4 | latitude, degrees * `1e7`, little-endian |
| lon_e7 | `int32` | 8 | longitude, degrees * `1e7`, little-endian |
| crc16 | `uint16` | 12 | CRC-16/CCITT-FALSE over first 12 bytes, little-endian |

Packet length は 14 byte です。

## `flags`

| bit | meaning |
| ---: | --- |
| 0 | `valid` |
| 1 | `usb_present` |
| 2 | `read_busy` |
| 3 | `read_error` |
| 4-7 | reserved, send `0` |

## `acs io`

`acs io` の受信 format として `packetufv1` / `PacketUFv1` を指定できます。

```bash
acs io -i /dev/ttyUSB1@921600,hex+packet,packetufv1
```
