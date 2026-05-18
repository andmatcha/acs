# `output/formats` モジュール

## 責務

- 各シリアル出力フォーマットの具体実装をまとめる層です。
- フォーマット共通で使う CRC 計算もここに置かれています。

## 主な処理

- `packetacv6` で compact report を 39 バイトの `PacketACv6` パケットへ変換します。
- `packetacv6usb` で `USB_READ` flag だけを立てた中立 `PacketACv6` ダミーパケットを生成します。
- `packetmv1` で compact report から生成した `PacketACv6` を XBee 送信用の `PacketMv1` へ削減します。
- `packetiv1` / `packetbv1` で XBee 送信用に削減した `PacketACv6` 系ダミーフレームを生成します。
- `packetgcv1` で compact report をアンテナサーボ向けの 9 バイト `PacketGCv1` コマンドへ変換します。
- `packetjfv1` で `PacketJFv1` のダミーフレームを生成します。
- `packetufv2` で `PacketUFv2` のダミーフレームを生成し、`control --monitor` / `io -i` の受信デコーダで 40 バイトの UF v2 text feedback を扱います。
- `crc.rs` で CRC16-CCITT-FALSE を計算します。

## 実装の要点

- `mod.rs` で format 名と実装を対応付けるレジストリ風の構成になっています。
- フォーマットごとの差分はサブディレクトリに閉じ込め、上位層は `OutputDriver` 越しに扱います。
- `packetgcv1` は `acs control` 用に HOME / STOP / MANUAL_POSITION / MANUAL_RATE だけを出力します。
  - 位置値は 0.1 度単位で、角度範囲は `0..270` 度です。HOME は中央の `135.0` 度を送ります。
- `packetjfv1` / `packetufv2` は現状ダミー送信専用で、compact からの一般エンコードは未対応です。
