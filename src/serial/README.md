# `serial` モジュール

## 責務

- シリアルポートの列挙、選択、監視、書き込みをまとめて扱う I/O 層です。

## 主な処理

- 使用可能なポート一覧を取得します。
- ポート未指定時は USB シリアルらしい候補を優先して自動選択します。
- `SerialMonitor` で reader thread を起動し、受信データをコールバック経由で通知します。
- `SerialWriter` で再試行付きの書き込みを行います。
- `open_monitor_and_writer` で同じポートを監視しながら送信するための共有接続を作れます。
- `SerialLineBuffer` で改行単位の入力再構成も提供します。

## 実装の要点

- macOS の `/dev/tty.*` と `/dev/cu.*` を同一グループとして扱い、自動選択時は dialout 向きの `/dev/cu.*` を優先します。
- reader thread は OS から細切れに届いたデータを `bytes_to_read()` で可能な範囲までまとめ、イベント数を抑えています。
- 読み書きの一時的な `TimedOut` / `WouldBlock` は正常系として吸収し、致命的なエラーだけを上位へ通知します。
