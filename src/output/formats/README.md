# `output/formats` モジュール

## 責務

- 各シリアル出力フォーマットの具体実装をまとめる層です。
- フォーマット共通で使う CRC 計算もここに置かれています。

## 主な処理

- `packetacv6` で compact report を 39 バイトの `PacketACv6` パケットへ変換します。
- `packetjfv1` で `PacketJFv1` のダミーフレームを生成します。
- `crc.rs` で CRC16-CCITT-FALSE を計算します。

## 実装の要点

- `mod.rs` で format 名と実装を対応付けるレジストリ風の構成になっています。
- フォーマットごとの差分はサブディレクトリに閉じ込め、上位層は `OutputDriver` 越しに扱います。
- `packetjfv1` は現状ダミー送信専用で、compact からの一般エンコードは未対応です。
