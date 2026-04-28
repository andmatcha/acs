# `pipeline/modules` モジュール

## 責務

- パイプラインで使う各処理モジュールの具体実装を持ち、設定から trait object を生成します。

## 主な処理

- Filter: `AllowAll`
- Transform: `Identity`、`Ds4ToCompact`、`OutputEncode`
- Classify: `None`
- Router: `Broadcast`、`SourceMap`

## 実装の要点

- `build_filter` / `build_transform_chain` / `build_classifier` / `build_router` が設定と実装を橋渡しします。
- `Ds4ToCompact` は DS4 の raw report を compact 8 バイトへ正規化します。
- `OutputEncode` は compact 8 バイトを前提にしており、長さ不一致は明確なエラーにします。
- `SourceMap` ルータは入力 ID ごとの明示ルーティングを扱い、該当が無い場合は `default_outputs` へフォールバックします。
