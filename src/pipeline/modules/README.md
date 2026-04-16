# `pipeline/modules` モジュール

## 責務

- パイプラインで使う各処理モジュールの具体実装を持ち、設定から trait object を生成します。

## 主な処理

- Filter: `AllowAll`、`DropEmpty`、`MatchSource`、`MatchPrefix`
- Transform: `Identity`、`Ds4ToCompact`、`OutputEncode`、`JoinLatest`
- Classify: `None`、`BySource`、`TagStatic`、`MatchPrefix`
- Router: `Broadcast`、`RoundRobin`、`SourceMap`、`TagBased`

## 実装の要点

- `build_filter` / `build_transform_chain` / `build_classifier` / `build_router` が設定と実装を橋渡しします。
- `JoinLatest` は入力 ID ごとの最新ペイロードを保持し、入力の順序を保った結合メッセージを生成します。
- `OutputEncode` は compact 8 バイトを前提にしており、長さ不一致は明確なエラーにします。
- `TagBased` ルータでは重複出力を避けるため、出力 ID の追加に重複除去ロジックを使っています。
