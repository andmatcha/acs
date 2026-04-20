# acs route 実装計画

## 目的

`acs route` を、複数のシリアル入力と複数のシリアル出力の間でデータを柔軟に中継できる汎用コマンドとして追加する。

満たしたい要件は次のとおり。

- 入力ポートを `--input-port` / `-i`、出力ポートを `--output-port` / `-o` で指定できる
- 処理順を `入力 -> フィルター -> 変換 -> 分類 -> 振り分け -> 出力` で固定しつつ、中間処理は差し替え可能にする
- `1入力1出力`、`1入力2出力`、`2入力1出力`、`2入力2出力` などを同じ仕組みで扱えるようにする
- そのまま出力、変換して出力、複数入力の到着順マージ、交互出力、複数入力を組み合わせて変換して複数出力へ複製、などを設定で表現できるようにする
- 使う変換モジュールや振り分け戦略を含め、設定はすべて `acs.config.json` に記述する
- `control` / `monitor` / `route` の間で、同じような機能を複数箇所に実装しない構成にする

## 現状整理

既存実装から見ると、今回の `route` は次の延長線上に置くのが自然。

- `src/app/cli/control.rs`
  - `DUALSHOCK 4 -> 変換 -> 1シリアル出力`
- `src/app/cli/monitor.rs`
  - `1個以上のシリアル入力 -> 監視`
- `src/serial/mod.rs`
  - `SerialMonitor` と `SerialConnection` がすでにあり、複数入力監視と単一出力書き込みの基盤は存在する
- `src/app/cli/config.rs`
  - `acs.config.json` を自前パーサで読んでおり、`control` / `monitor` の typed config へ変換している
- `src/app/cli/serial_dashboard.rs`
  - 入出力ポートの可視化とログ保存を再利用できる

つまり `route` で新たに必要なのは、主に「複数入力を受けて、段階的に処理し、複数出力へ振り分けるランタイム層」である。
同時に、`control` と `monitor` を含めた共通土台を先に整理しないと、似たイベントループや adapter、dashboard 反映処理がコマンドごとに増える可能性が高い。

## 設計方針

### 1. 最優先は「機能ごとに実装を 1 か所へ寄せる」こと

今回の設計では、`route` を足すこと自体よりも、`control` / `monitor` / `route` の間に同種の実装を増やさないことを重視する。

原則:

- コマンドごとに独自の event loop を持たない
- コマンドごとに独自の serial 入力監視ラッパを持たない
- コマンドごとに独自の dashboard 更新処理を持たない
- コマンドごとに独自の module registry を持たない
- コマンドごとに独自の config 解決ロジックを持たない

各責務の単一実装先:

- 入出力の監視とセッション進行
  - `session` 層
- filter / transform / classify / router
  - `pipeline` 層
- serial / ds4 などの device 差分
  - adapter / sink
- CLI 引数から正規化済み設定への変換
  - CLI / config 正規化層

この方針に従い、各コマンドは「共通層の薄いフロントエンド」に留める。

### 2. `route` は「パイプラインの集合」として実装する

`route` 全体を 1 本の固定ロジックで作るのではなく、複数の `pipeline` を登録できる形にする。

各 `pipeline` は以下を持つ。

- 対象入力 ID の一覧
- `filter` モジュール
- `transform` モジュール
- `classify` モジュール
- `route` モジュール

1 つの入力イベントは、その入力を購読している全 `pipeline` に流す。

これにより次の表現がしやすい。

- 同じ入力を 2 つの出力先へ別条件で流す
- 同じ入力を片方は素通し、片方は変換して出す
- 2 入力を 1 つの pipeline で結合しつつ、別 pipeline では個別にも出す

### 3. ランタイムは「単一イベントループ」で処理する

各入力ポートは既存の `SerialMonitor` で監視し、共通の channel に `SerialEvent` を送る。
メインスレッド側でイベントを 1 件ずつ取り出し、各 pipeline を順番に実行する。

この構成にする理由は次のとおり。

- 複数入力の「来た順マージ」を自然に表現しやすい
- `round_robin` や `alternate` など状態を持つ振り分けを実装しやすい
- 変換モジュールが「直前に来た別入力の値を保持する」ような処理を書きやすい
- 余計な lock を増やさずに済む

### 4. モジュールは動的ロードではなく「文字列名で選ぶ組み込み registry」から始める

要件では「どの変換モジュールを使うかを JSON に書く」ことが重要であり、必ずしも OS レベルの動的プラグインは必要ではない。

初期実装では次の方式を推奨する。

- Rust 側で利用可能なモジュールを registry に登録する
- `acs.config.json` では `"module": "identity"` のように文字列名で指定する
- モジュール固有パラメータも同じ object 内に持たせる

これなら実装コストを抑えつつ、利用者から見れば「JSON でモジュールを切り替える」要件を満たせる。

## 推奨アーキテクチャ

### 1. 追加する主なファイル

- `src/session/mod.rs`
  - 共通セッション層の入口
- `src/session/runtime.rs`
  - event loop、input polling、dashboard/log 反映
- `src/session/input.rs`
  - `InputAdapter` と実装群
- `src/session/output.rs`
  - `OutputSink` と実装群
- `src/session/dashboard.rs`
  - dashboard 表示と session からの橋渡し
- `src/session/event.rs`
  - `IngressFrame` などの共通イベント表現
- `src/pipeline/mod.rs`
  - 共通 pipeline 層の入口
- `src/pipeline/config.rs`
  - 正規化済み pipeline 設定
- `src/pipeline/engine.rs`
  - pipeline 実行本体
- `src/pipeline/message.rs`
  - 中間メッセージ表現
- `src/pipeline/modules/mod.rs`
  - モジュール registry
- `src/pipeline/modules/filter.rs`
- `src/pipeline/modules/transform.rs`
- `src/pipeline/modules/classify.rs`
- `src/pipeline/modules/router.rs`
- `src/app/cli/route.rs`
  - route CLI と route 設定の正規化
- `src/app/cli/monitor.rs`
  - monitor CLI と monitor 設定の正規化
- `src/app/cli/control.rs`
  - control CLI と control 設定の正規化

必要なら後続で以下も分離する。

- `src/app/cli/config_normalize.rs`
  - 各コマンド設定から `SessionSpec` / `PipelineSpec` への変換
- `src/pipeline/tests/...`
- `src/session/tests/...`

### 2. 共通メッセージ型

中間処理を差し替えやすくするため、各段階は共通のメッセージ表現を受け渡す。

想定する型のイメージ:

```rust
struct RouteMessage {
    pipeline_id: String,
    source_input_ids: Vec<String>,
    payload: Vec<u8>,
    timestamp_millis: u128,
    tags: Vec<String>,
    attributes: BTreeMap<String, String>,
}
```

ポイント:

- `source_input_ids`
  - 2入力を組み合わせて生成したメッセージでも由来を保持できる
- `tags`
  - classify の結果を router が参照できる
- `attributes`
  - モジュール間で軽いメタデータを受け渡せる

入力直後の生イベントは別型にしてもよい。

```rust
struct IngressFrame {
    input_id: String,
    port: String,
    bytes: Vec<u8>,
    sequence: u64,
}
```

`IngressFrame` は command ごとに別型を作らず、`session` 層から `pipeline` 層へ渡る唯一の入力表現にする。
これにより、serial でも DS4 でも downstream の処理を共通化できる。

### 3. モジュール境界

段階は固定しつつ、各段階の具体実装だけ差し替える。

```rust
trait FilterModule {
    fn accept(&mut self, frame: &IngressFrame) -> Result<bool, String>;
}

trait TransformModule {
    fn transform(&mut self, frame: &IngressFrame) -> Result<Vec<RouteMessage>, String>;
}

trait ClassifyModule {
    fn classify(&mut self, message: &mut RouteMessage) -> Result<(), String>;
}

trait RouterModule {
    fn route(&mut self, message: &RouteMessage) -> Result<Vec<String>, String>;
}
```

補足:

- `TransformModule` は `0..N` 件の `RouteMessage` を返せるようにする
  - drop
  - 1入力1出力
  - 1入力2出力用の複製
  - 2入力を受けて 1 件生成
  - 1入力から複数派生
  を同じ形で扱える
- `RouterModule` は最終的な出力 ID 一覧を返す
  - `broadcast`
  - `round_robin`
  - `source_map`
  - `tag_based`
などを実装しやすい

### 4. 2入力を組み合わせるための stateful transform

「複数入力を組み合わせて変換して出力」は router ではなく transform の責務に置く。

理由:

- 「組み合わせて新しい payload を作る」のは経路選択ではなく変換そのものだから
- transform に状態を持たせれば `latest_a + latest_b`、`a を受けたら b と結合`、`両方そろったら出力` を表現しやすいから

初期実装で想定する代表モジュール:

- `identity`
  - そのまま通す
- `join_latest`
  - 複数入力の最新値を保持し、そろったら結合して出力
- `concat`
  - 到着した bytes を指定順で連結
- `output_encode`
  - 既存 `output::formats` に近い変換を route 用に再利用できる形へ移す

### 5. 分類と振り分けを分離する

要件上は「分類してから振り分け」なので、分類結果は `tags` や `attributes` に積む。

例:

- `classify.module = "by_source"`
  - `source:a`, `source:b` のような tag を付与
- `classify.module = "match_prefix"`
  - payload 先頭バイトや文字列で tag を付与
- `route.module = "tag_based"`
  - `tag == control` なら `out_control`
  - `tag == telemetry` なら `out_telemetry`

この分離により、分類条件を保ったまま振り分け戦略だけ差し替えやすくなる。

### 6. CLI は薄く、正規化済み spec を共通層へ渡すだけにする

重複を避けるため、各 CLI が直接 runtime の詳細を組み立てないようにする。

各 CLI が持ってよい責務:

- コマンド固有オプションの解釈
- 既存 UX に沿った help
- config と CLI のマージ
- `SessionSpec` / `PipelineSpec` への正規化

各 CLI が持たない方がよい責務:

- event loop
- serial monitor の起動手順
- dashboard 更新の分岐
- pipeline 実行順
- module の生成分岐

この線引きにより、機能追加時の修正箇所を共通層に集中できる。

## 重複実装を避けるための具体ルール

### 1. 共通化の単位は「機能」単位にする

次のような「似ているが少し違う」実装を各コマンド配下に増やさない。

- `control` 専用 event loop
- `monitor` 専用 event loop
- `route` 専用 event loop
- `control` 専用 dashboard 更新
- `route` 専用 dashboard 更新
- `monitor` 専用 input adapter 管理

違いは branch ではなく spec と module に押し込む。

### 2. config は 1 度だけ正規化する

`acs.config.json.control`、`acs.config.json.monitor`、`acs.config.json.route` は形が違ってもよいが、runtime の入口では共通の型へ変換し終えている状態にする。

想定する共通型:

```rust
struct SessionSpec { /* inputs, observers, outputs, raw, display, log_dir ... */ }
struct PipelineSpec { /* pipelines, modules, routing ... */ }
```

こうしておくと runtime 側は「どのコマンドから来た設定か」を意識しなくてよい。

### 3. serial / ds4 の差分は adapter に閉じ込める

`control` だけが DS4 を扱うとしても、main loop に `if control { ... }` を増やさない。

差分は次へ閉じ込める。

- `InputAdapter`
- `OutputSink`
- module registry

### 4. dashboard と logging は session 層の責務に固定する

各コマンドが独自に `SerialDashboard` を操作し始めると、表示更新や flush の挙動がすぐ重複する。
そのため dashboard 反映は session runtime に集約する。

### 5. module registry は 1 か所に集約する

`control` だけが知る transform、`route` だけが知る transform、という registry 分断は避ける。
`ds4_to_compact` や `output_encode` も pipeline registry に登録し、使うかどうかだけを command spec で切り替える。

### 6. 既存コードの移行時は「先に共通化、後で置換」の順にする

重複を減らしたい場合でも、いきなり 3 コマンドを同時に書き換えない。
まず共通層を作り、その後で既存コマンドを順番に載せ替える。

## `acs.config.json` の推奨スキーマ

### 基本形

```json
{
  "route": {
    "raw": true,
    "inputs": [
      {
        "id": "in_a",
        "port": "/dev/ttyUSB0",
        "baud": 115200
      },
      {
        "id": "in_b",
        "port": "/dev/ttyUSB1",
        "baud": 115200
      }
    ],
    "outputs": [
      {
        "id": "out_main",
        "port": "/dev/ttyUSB2",
        "baud": 115200
      },
      {
        "id": "out_sub",
        "port": "/dev/ttyUSB3",
        "baud": 115200
      }
    ],
    "pipelines": [
      {
        "id": "merge_passthrough",
        "inputs": ["in_a", "in_b"],
        "filter": { "module": "allow_all" },
        "transform": { "module": "identity" },
        "classify": { "module": "by_source" },
        "route": {
          "module": "broadcast",
          "outputs": ["out_main"]
        }
      },
      {
        "id": "paired_to_both",
        "inputs": ["in_a", "in_b"],
        "filter": { "module": "allow_all" },
        "transform": {
          "module": "join_latest",
          "separator_hex": "2c"
        },
        "classify": { "module": "tag_static", "tags": ["paired"] },
        "route": {
          "module": "broadcast",
          "outputs": ["out_main", "out_sub"]
        }
      }
    ]
  }
}
```

### CLI 上書きルール

`route` も既存コマンドと同じく「CLI 引数が config より優先」にする。

推奨する引数:

- `--input-port` / `-i`
- `--output-port` / `-o`
- `--config`
- `--log-dir`
- `--display input:...=...+line|packet`

複数ポートを扱うので、CLI では `ID=PORT` 形式を推奨する。

```bash
acs route \
  -i in_a=/dev/ttyUSB0 \
  -i in_b=/dev/ttyUSB1 \
  -o out_main=/dev/ttyUSB2 \
  -o out_sub=/dev/ttyUSB3
```

単一入力・単一出力だけ使う場合は、将来的に `-i /dev/ttyUSB0 -o /dev/ttyUSB1` も許容してよいが、複数系を考えると `ID=PORT` を正規形にした方が曖昧さが少ない。

## 初期実装で入れる builtin module 候補

### filter

- `allow_all`
- `drop_empty`
- `match_source`
- `match_prefix`

### transform

- `identity`
- `concat`
- `join_latest`
- `output_encode`

### classify

- `none`
- `by_source`
- `tag_static`
- `match_prefix`

### route

- `broadcast`
- `round_robin`
- `alternate`
- `source_map`
- `tag_based`

この最小セットで、要件にある多くのケースを表現できる。

## 要件とモジュール対応

### そのまま出力

- `filter = allow_all`
- `transform = identity`
- `classify = none`
- `route = broadcast`

### 変換して出力

- `transform = output_encode` など

### 複数入力を来た順に 1 出力

- 複数入力を同じ pipeline に登録
- ランタイムを単一イベントループで実行
- `transform = identity`
- `route = broadcast(outputs=[out_main])`

### 交互に出力

- `route = round_robin` または `alternate`

### 複数入力を組み合わせて変換し、複数出力へ同じように出力

- `transform = join_latest`
- `route = broadcast(outputs=[out_a, out_b])`

## 実装ステップ

### Phase 1: `session` 共通層を抽出する

- `InputAdapter`
- `OutputSink`
- 共通 event loop
- dashboard / log の共通反映
- `SessionSpec`

完了条件:

- `monitor` の現在の挙動を `session` 層だけで再現できる
- command ごとの event loop が不要になる

### Phase 2: `monitor` を `session` ベースへ寄せる

- `src/app/cli/monitor.rs` から session 以外の実行ロジックを減らす
- display / raw / log / pause の挙動を session 側へ寄せる

完了条件:

- `monitor` が薄い CLI ラッパになる
- 既存 monitor 機能が維持される

### Phase 3: `pipeline` 共通層を追加する

- `PipelineEngine`
- `PipelineSpec`
- `RouteMessage`
- module registry
- `identity` / `broadcast`

完了条件:

- `session + pipeline` の最小構成で `1入力1出力` が動く
- `identity + broadcast` を共通 module として呼べる

### Phase 4: `route` CLI と route config を追加する

- `src/app/cli/mod.rs` に `route` を追加
- `src/app/cli/help.rs` に help を追加
- `src/app/cli/config.rs` に `RouteConfig` の読み込みを追加
- `RouteConfig -> SessionSpec + PipelineSpec` の正規化を追加
- `acs.config.example.json` と `README.md` に `route` 例を追加

完了条件:

- `acs route --help` が表示できる
- `acs.config.json` から `route` セクションを読める
- `1入力2出力`、`2入力1出力`、`2入力2出力` が設定で表現できる

### Phase 5: stateful transform / router を入れる

- `join_latest`
- `round_robin`
- `source_map`
- `tag_based`

完了条件:

- 到着順マージ
- 交互出力
- 2入力結合後の複数出力複製
  が動作する

### Phase 6: `control` を `session + pipeline` ベースへ寄せる

- `Ds4InputAdapter`
- `ds4_to_compact`
- `output_encode`
- `control` 固有 config の正規化
- `control --monitor` を observer input として整理

完了条件:

- 既存 `control` の UX を維持したまま、実行本体が共通層へ載る
- `control` 固有の変換も pipeline registry に乗る

### Phase 7: テストとドキュメントを固める

- config parse unit test
- module 単体 test
- session / pipeline の pure test
- README の利用例追加

完了条件:

- 代表的な monitor / route / control パターンが共通層経由で再現できる

## テスト方針

`SerialMonitor` / `SerialConnection` に直接依存したままだとテストしづらいので、route コアは serial I/O から切り離しておく。

推奨:

- route コアは `IngressFrame -> DispatchPlan` の純粋ロジックとしてテスト可能にする
- シリアル実機依存は薄い adapter に閉じ込める
- 代表ケースを unit test 化する

最低限ほしいテスト:

- `monitor` が `pipeline_engine = None` でも動くこと
- config の必須項目不足、未知 module 名、存在しない input/output ID の検出
- `identity + broadcast`
- `round_robin`
- `join_latest`
- `by_source + source_map`
- 同一入力を複数 pipeline に流したときの独立性
- `control` の `ds4_to_compact -> output_encode(format=packetacv6)` 連鎖

## 既存実装に合わせた注意点

### 1. JSON パーサは当面そのまま拡張する

現状は `serde_json` ではなく自前の最小 JSON パーサを使っている。
今回の計画では、まずはその方針に合わせて `route` 用の typed parser を追加する前提とする。

ただし `route` の config は `control` / `monitor` より入れ子が深くなるため、以下の helper を先に追加した方がよい。

- object 必須取得
- array 必須取得
- enum 文字列取得
- ID 重複チェック

### 2. dashboard は再利用しつつ header を route 向けに増やす

既存の `SerialDashboard` は input/output 両方を扱えるので再利用可能。
ただし `route` は pipeline 情報も出したくなるため、header には次を表示したい。

- 入力一覧
- 出力一覧
- 有効 pipeline 一覧
- raw/line モード
- log path

### 3. エラー時はまず fail-fast を採用する

初期実装では複雑な自動復旧よりも、設定ミスや module 実行失敗を明確に止める方が安全。

将来的には以下を追加できる。

- pipeline 単位で drop して続行
- 一部出力のみエラー時でも他出力は継続
- 再接続

## control との共通化検討

### 結論

`acs control` はかなりの部分を `route` 系の汎用モジュールへ寄せられる。
ただし、より自然なのは「`control` が `route` を呼ぶ」構造よりも、「`control` と `route` が同じ汎用 pipeline runtime を共有する」構造である。

言い換えると、共通化の土台は `route` コマンドそのものではなく、その下に置く `flow` / `pipeline` 層にした方がよい。

### 現在の control を責務で分解すると

既存 `src/app/cli/control.rs` の処理は、おおむね次の 5 層に分けられる。

- `入力取得`
  - `Ds4Controller::open`
  - `read_next_report`
- `入力変換`
  - `input::compact::convert_input_report`
- `出力変換`
  - `output::formats::OutputDriver`
  - 現状は `packetacv6`
- `出力配送`
  - `SerialConnection::write_bytes`
- `監視/UI`
  - `SerialDashboard`
  - `open_serial_monitors`
  - ログ

このうち `control` 固有なのは主に「DS4 を読む入口」と「controller 選択 UI」であり、それ以外は route 側の汎用 runtime に寄せやすい。

### そのまま共有しやすいもの

- `SerialConnection`
- `SerialMonitor`
- `SerialDashboard`
- logging 周辺
- `output::formats::packetacv6`
  - route の transform module として移植しやすい
- `input::compact::convert_input_report`
  - route の transform module として移植しやすい

特に `control` の中核処理は本質的に

`DS4 report -> compact report -> PacketACv6 packet -> serial output`

なので、これは route/pipeline の「入力 adapter + transform chain + broadcast router」の 1 例として表現できる。

### control 固有として残るもの

- `DUALSHOCK 4` デバイス列挙と選択
- `--controller` オプション
- `controllers` サブコマンドとの整合
- 必要なら profile 変更時の音再生
  - `ModeSoundPlayer`

このあたりは serial routing とは責務が違うため、共通 runtime の外側に薄く残すのが自然。

### control を route の応用として成立させるために必要な条件

#### 1. route の入力を serial 専用にしない

現在の route 計画は serial 入出力を前提にしているが、`control` を乗せるなら入力 source を抽象化する必要がある。

想定する境界:

```rust
trait InputAdapter {
    fn id(&self) -> &str;
    fn poll(&mut self, timeout: Duration) -> Result<Vec<IngressFrame>, String>;
}
```

候補:

- `SerialInputAdapter`
- `Ds4InputAdapter`

これにより `route` は serial routing を実現しつつ、`control` は DS4 を同じ runtime へ流し込める。

#### 2. transform は 1 個ではなく chain にした方がよい

`control` を素直に表現すると、変換は 1 段ではなく少なくとも 2 段ある。

- `ds4_report_to_compact`
- `compact_to_packetacv6_packet`

このため、実装上は「transform ステージ 1 個」でも、その中に module 配列を持てる形が望ましい。

例:

```json
{
  "transform": {
    "modules": [
      { "module": "ds4_to_compact" },
      { "module": "output_encode", "format": "packetacv6" }
    ]
  }
}
```

もしくは runtime 上は `Vec<Box<dyn TransformModule>>` を順に通す。

この形にしておくと `control` だけでなく route でも応用しやすい。

#### 3. route module の前に「sink へ送らない observer」を許容すると使いやすい

既存 `control` はメイン出力ポート以外に monitor port をぶら下げてダッシュボードへ流している。

これは routing 対象の本流ではなく、観測用の side channel として扱うのが自然。

そのため runtime には次の 2 種類があるとよい。

- `pipeline input`
  - 変換と振り分けの対象
- `observer input`
  - 表示とログのみ

こうしておくと、今の `control --monitor` をきれいに載せ替えられる。

### 推奨する最終像

最も収まりがよいのは次のレイヤリング。

- `src/pipeline/...`
  - 入力 adapter
  - filter / transform / classify / router
  - runtime
- `src/app/cli/route.rs`
  - serial routing 用 CLI
  - `--input-port` / `--output-port`
  - `acs.config.json.route`
- `src/app/cli/control.rs`
  - DS4 向け CLI
  - `--controller`
  - `acs.config.json.control`
  - 内部では pipeline runtime を使う

この形なら、利用者に見える `control` の UX は維持しつつ、実装は route 系の汎用モジュールへ寄せられる。

### control を route 設定で表すとどうなるか

概念上は次のように表現できる。

```json
{
  "control": {
    "controller": "0",
    "output": {
      "id": "main",
      "port": "/dev/ttyUSB0",
      "baud": 115200
    },
    "pipeline": {
      "inputs": ["ds4_main"],
      "filter": { "module": "allow_all" },
      "transform": {
        "modules": [
          { "module": "ds4_to_compact" },
          { "module": "output_encode", "format": "packetacv6" }
        ]
      },
      "classify": { "module": "none" },
      "route": {
        "module": "broadcast",
        "outputs": ["main"]
      }
    }
  }
}
```

つまり `control` は「DS4 入力を使う route preset」として十分表現可能である。

### どの程度共有できるか

責務ベースで見ると、`control` の 7 割以上は共通化可能と考えてよい。

共通化しやすい部分:

- runtime
- pipeline 実行順
- serial 出力
- dashboard / log
- 変換 module
- routing module

個別に残すべき部分:

- DS4 デバイス選択
- `control` 固有 CLI
- `control` 固有 config から共通 pipeline config への変換

### 実装上のおすすめ

段階的には以下が安全。

1. `session` 共通層を先に抽出する
2. `monitor` を `session` ベースへ寄せる
3. `pipeline` 共通層を追加する
4. `acs route` を `session + pipeline` で実装する
5. `acs control` の変換処理を module 化して pipeline へ寄せる
6. 最後に `control` の実行本体を `session + pipeline` ベースへ置き換える

この順番なら、既存 `monitor` と `control` の動作を壊しにくく、重複実装も増やさずに済む。

## monitor との共通化検討

### 結論

`acs monitor` とも共通層は切り出した方がよい。
ただし `monitor` は `route` や `control` と違って `filter -> transform -> classify -> route -> output` を必須にはしないため、共通化の中心は pipeline 層そのものではなく、その一段下の `I/O session` 層になる。

整理すると、共通層は 2 段に分けるのがよい。

- `I/O session` 層
  - 入力 adapter の起動
  - 共通 event loop
  - dashboard / log
  - observer input
  - signal handling
- `pipeline` 層
  - filter
  - transform chain
  - classify
  - router
  - sink への dispatch

この形なら `monitor` は `I/O session` だけを使い、`route` と `control` は `I/O session + pipeline` を使える。

### 現在の monitor を責務で分解すると

既存 `src/app/cli/monitor.rs` の処理は、おおむね次の 4 層に分けられる。

- `入力取得`
  - `open_serial_monitors`
  - `SerialMonitor`
- `イベント待ち合わせ`
  - 共通 channel
  - render interval と wait interval
- `監視/UI`
  - `SerialDashboard`
  - `raw` / line モード
  - display 設定
- `ログ`
  - dashboard 経由で保存

つまり `monitor` は「serial input を集約して表示するセッション」であり、route の pipeline 処理を持たない special case と見なせる。

### そのまま共有しやすいもの

- `SerialMonitor`
- `SerialDashboard`
- `make_serial_callback`
- event channel の扱い
- render loop
- raw / line の入力整形
- logging

これらは `monitor` だけのロジックではなく、`route` や `control` でもそのまま必要になる。

### monitor 固有として残るもの

- `--port`
- `--baud`
- `monitor` 用 header 文言
- 「出力を持たない」というコマンド上の意味づけ

このため `monitor` 自体は薄い CLI ラッパにし、実装本体は共通 `I/O session` に寄せられる。

### monitor を載せるために追加で切り出したい共通層

#### 1. `SessionRuntime`

`control` / `route` / `monitor` すべてに共通するのは「複数の入力 source を起動し、イベントを受け、dashboard と log を更新しながら、必要なら pipeline に流す」という流れである。

そのため、次のような土台を先に作るのがよい。

```rust
struct SessionRuntime {
    inputs: Vec<Box<dyn InputAdapter>>,
    observers: Vec<Box<dyn InputAdapter>>,
    output_sinks: Vec<Box<dyn OutputSink>>,
    pipeline_engine: Option<PipelineEngine>,
    dashboard: DashboardRuntime,
}
```

考え方:

- `monitor`
  - `pipeline_engine = None`
  - inputs をそのまま dashboard/log へ流す
- `route`
  - serial inputs + serial outputs + `pipeline_engine`
- `control`
  - DS4 input + serial output + observer serial inputs + `pipeline_engine`

#### 2. `InputAdapter` / `OutputSink` の抽象

`monitor` も含めて共通化するなら、入力も出力も adapter 化しておくのが自然。

候補:

- `SerialInputAdapter`
- `Ds4InputAdapter`
- `SerialOutputSink`

これによりコマンドごとの差は「どの adapter を何本作るか」へ縮まる。

#### 3. `DashboardRuntime`

今の `SerialDashboard` はすでに再利用しやすいが、さらに共通化するなら次をまとめるとよい。

- header 構築
- port ごとの display mode 設定
- event 反映
- render タイミング
- pause/resume
- flush

すると各 CLI は「どの port を input/output 扱いにするか」を宣言するだけで済む。

### 推奨する最終像の更新

`control` と `route` だけでなく `monitor` も含めるなら、最終的なレイヤリングは次がよい。

- `src/session/...`
  - `InputAdapter`
  - `OutputSink`
  - serial / ds4 adapter
  - event loop
  - dashboard / log
- `src/pipeline/...`
  - filter
  - transform chain
  - classify
  - router
  - dispatch plan
- `src/app/cli/monitor.rs`
  - `session` のみ利用
- `src/app/cli/route.rs`
  - `session + pipeline` を利用
- `src/app/cli/control.rs`
  - `session + pipeline` を利用

この構成にすると、`monitor` は pipeline のない route/control として扱える。

### monitor を共通層へ寄せたときの位置付け

概念上の位置付けは次の通り。

- `monitor`
  - `観測専用セッション`
- `route`
  - `serial input/output を持つ汎用 pipeline セッション`
- `control`
  - `DS4 input を持つプリセット pipeline セッション`

この整理にすると、3 コマンドの関係がかなり明確になる。

### 実装上のおすすめ更新

3 コマンド全部を見据えるなら、実装順は次が安全。

1. `session` 共通層を抽出する
2. 既存 `monitor` を `session` ベースへ寄せる
3. `pipeline` 共通層を追加する
4. `acs route` を `session + pipeline` で実装する
5. `acs control` を `session + pipeline` ベースへ寄せる

この順番だと、一番単純な `monitor` から共通土台を検証できるので、移行リスクを下げやすい。

## 確認したい点

### 1. `-i` / `-o` の入力形式

複数入力・複数出力を前提にすると、CLI は `ID=PORT` 形式を正規形にしたい。

例:

```bash
acs route -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_main=/dev/ttyUSB2
```

この方針で問題ないか確認したい。

### 2. モジュールは初期段階では「組み込み registry」でよいか

JSON から文字列で選べれば要件は満たせるが、`.so` / `.dylib` のような動的プラグインまで最初から必要かは確認したい。
初期段階は組み込み registry を推奨する。

### 3. 対象 I/O はまず serial only でよいか

既存要件からは serial routing が主目的に見えるため、初期実装は serial input / serial output に絞る想定。
必要なら将来的に HID や file/socket を input adapter として足せる構造にする。

## 推奨する最初の実装スコープ

最初の 1 本目としては、次の範囲で着手するのが安全。

- serial input / serial output のみ
- `allow_all`
- `identity`
- `join_latest`
- `by_source`
- `broadcast`
- `round_robin`
- `source_map`

この範囲でも、要件に挙がっている主要パターンの大半をカバーできる。
