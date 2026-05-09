# `output/formats/packetacv6` モジュール

## 責務

- DS4 由来の compact report を、ARES 向けの `PacketACv6` フレームへ変換します。

## 主な処理

- `definition.rs` でパケット長、しきい値、中立電流値、各プロファイルの定数を定義します。
- `encoder.rs` で compact report の各入力を電流値や制御ビットへ変換し、CRC 付きの 39 バイトパケットを組み立てます。
- `reduced.rs` で `PacketACv6` を XBee 送信用の `PacketMv1` / `PacketIv1` / `PacketBv1` へ削減します。
- `mod.rs` で `PacketACv6` driver、`USB_READ` 用の中立ダミー generator、`PacketACv6` driver 出力を削減する `PacketMv1` driver を提供します。
- `sound.rs` でプロファイル切替時の効果音再生を扱います。

## 実装の要点

- エンコーダは `seq`、enable 状態、現在プロファイル、押下エッジ判定用の前回状態を保持する状態付き実装です。
- `OPTIONS` ボタンで有効/無効をトグルし、`SHARE` ボタンで `normal` / `power` / `sensitive` を巡回させます。
- 出力無効時は全モータ電流を中立値へ戻すため、安全側の振る舞いが明示されています。
- 縮小パケットは削減後の byte 列に対して CRC を再計算します。
- 効果音再生は `afplay` が見つかったときだけ動き、音声ファイルが無い場合も致命的エラーにはしません。
