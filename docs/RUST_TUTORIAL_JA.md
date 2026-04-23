# acs のソースコードで学ぶ Rust チュートリアル

## はじめに

この文書は、Rust を初めて学ぶ人のためのチュートリアルです。特に、C や Python を少し学んだことがあり、「文法は読めても、所有権やライフタイムで止まってしまう」という人を主な読者として想定しています。

教材として使うのは、このリポジトリにある `acs` の実装です。`acs` はシリアル通信、HID 入力、ターミナル UI、設定ファイル読み込み、並行処理などを含む、Rust 学習用としてかなり良い題材です。小さすぎて抽象的になりすぎることもなく、巨大すぎて追えなくなることもありません。

このチュートリアルでは、できるだけ実際のソースコードを引用しながら説明します。文法だけを孤立して覚えるのではなく、「この文法はこの実装でこう役立っている」という感覚を掴むことを目標にします。

## 読み方

- まずは前から順に読んでください。
- コード例は、完全に暗記する必要はありません。
- 分からない部分が出たら、該当ファイルを開いて前後の文脈を見るのがおすすめです。
- 特に第 6 章の所有権、第 10 章のライフタイムは、一度で完全理解できなくても大丈夫です。

## 目次

1. Rust プロジェクトの入口とモジュール
2. 変数、関数、式、制御構文
3. 型、配列、スライス、コレクション
4. 構造体、列挙型、パターンマッチ
5. `Option` と `Result` とエラー処理
6. 所有権、借用、ミュータブル参照
7. `impl`、メソッド、関連関数
8. トレイトと trait object
9. ジェネリクス、クロージャ、イテレータ
10. ライフタイム
11. 並行処理と共有状態
12. `unsafe`、FFI、条件付きコンパイル
13. テストとビルドスクリプト
14. まとめと次の学習

---

## 第1章 Rust プロジェクトの入口とモジュール

Rust のプログラムは、基本的には crate という単位で構成されます。`acs` では `Cargo.toml` が crate の設定ファイルで、`src/main.rs` が実行可能ファイルの入口です。

`src/main.rs` はとても短く、全体の骨格をよく表しています。

出典: `src/main.rs`

```rust
mod app;
mod common;
mod input;
mod output;
mod pipeline;
mod port_display;
mod serial;
mod session;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::cli::run()
}
```

ここから分かることは多いです。

- `mod app;` のような宣言は、「`src/app.rs` あるいは `src/app/mod.rs` に対応するモジュールを読み込む」という意味です。
- `use std::process::ExitCode;` は名前の導入です。C の `#include` ではなく、「この名前を短く書けるようにする」イメージです。
- `fn main() -> ExitCode` と書けるので、`main` は整数だけでなく `ExitCode` 型も返せます。

`src/app/cli/mod.rs` では、さらにサブコマンドごとに処理を分岐しています。

出典: `src/app/cli/mod.rs`

```rust
pub fn run() -> ExitCode {
    let mut args = env::args();
    let bin_name = args.next().unwrap_or_else(|| String::from("acs"));

    match args.next().as_deref() {
        Some("--help") | Some("-h") => {
            help::print_help(&bin_name);
            ExitCode::SUCCESS
        }
        Some("control") => control::run(args.collect(), &bin_name),
        Some("monitor") => monitor::run(args.collect(), &bin_name),
        Some("route") => route::run(args.collect(), &bin_name),
        Some("send") => send::run(args.collect(), &bin_name),
        None => {
            help::print_help(&bin_name);
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown subcommand: {command}");
            help::print_usage(&bin_name);
            ExitCode::from(2)
        }
    }
}
```

このコードは、Rust のモジュールの使い方だけでなく、`match`、`Option`、借用、文字列処理も同時に示しています。Rust のコードはこのように、1 つの短い関数に複数の学習要素が詰まっていることが多いです。

### `pub` と `pub(crate)`

このプロジェクトでは `pub` より `pub(crate)` がよく使われています。これは「この crate の内部からは見えるが、外部 crate には公開しない」という意味です。C で言えば「ファイルローカルとグローバルの中間」、Python で言えば「慣習上の非公開よりは強いが、完全公開ではない」くらいの感覚です。

---

## 第2章 変数、関数、式、制御構文

Rust の変数は、デフォルトで不変です。再代入したいときだけ `mut` を付けます。

```rust
let args = env::args();      // 不変
let mut args = env::args();  // 可変
```

これは C や Python とかなり違う感覚です。最初は少し窮屈に見えますが、「どこで値が変わるか」が明示されるので、コードが読みやすくなります。

### 関数宣言

Rust の関数は `fn` で定義します。

出典: `src/input/compact.rs`

```rust
pub fn convert_input_report(report: &[u8]) -> Result<CompactReport, CompactError> {
    let hid10 = normalize_input_report(report)?;
    convert_hid10(&hid10)
}
```

ここでは次の点が重要です。

- 引数 `report: &[u8]` は「`u8` のスライスへの共有参照」です。
- 戻り値 `Result<CompactReport, CompactError>` は「成功時は `CompactReport`、失敗時は `CompactError`」です。
- 最後の行にセミコロンがありません。Rust では、ブロックの最後の式が戻り値になります。

### `if` と `match`

Rust の `if` は文ではなく式です。つまり値を返せます。

出典: `src/input/compact.rs`

```rust
fn normalize_input_report(report: &[u8]) -> Result<[u8; NORMALIZED_LEN], CompactError> {
    if report.len() >= USB_REPORT_LEN {
        return Ok(report[..NORMALIZED_LEN]
            .try_into()
            .expect("slice has fixed size"));
    }

    if report.len() >= BLUETOOTH_REPORT_LEN {
        return Ok(report[..NORMALIZED_LEN]
            .try_into()
            .expect("slice has fixed size"));
    }

    Err(CompactError::ReportTooShort {
        expected: BLUETOOTH_REPORT_LEN,
        actual: report.len(),
    })
}
```

一方で、多分岐には `match` を多用します。

出典: `src/input/compact.rs`

```rust
let dpad_bits = match dpad {
    0 => 0b0001,
    1 => 0b0011,
    2 => 0b0010,
    3 => 0b0110,
    4 => 0b0100,
    5 => 0b1100,
    6 => 0b1000,
    7 => 0b1001,
    8 => 0,
    value => return Err(CompactError::InvalidDpad(value)),
};
```

`match` は C の `switch` より強力です。整数だけでなく、列挙型、`Option`、`Result`、スライス、タプルなどにも使えます。また、すべてのパターンを網羅する必要があるので、分岐漏れを減らせます。

### マクロ

Rust では `println!`、`format!`、`write!` のように `!` が付くものはマクロです。関数に見えますが、コンパイル時に展開されます。C のマクロと違ってかなり安全で、構文木レベルで扱われます。

出典: `build.rs`

```rust
println!("cargo:rustc-env=ACS_BUILD_COMMIT={commit}");
```

---

## 第3章 型、配列、スライス、コレクション

Rust は静的型付け言語です。Python と違って、変数の型はコンパイル時に決まります。ただし、多くの場面で型推論が効くので、毎回型名を書く必要はありません。

### 基本型と定数

出典: `src/input/compact.rs`

```rust
const BLUETOOTH_REPORT_LEN: usize = 10;
const USB_REPORT_LEN: usize = 64;
const COMPACT_LEN: usize = 8;

pub type CompactReport = [u8; COMPACT_LEN];
```

ここで使われている型は次の通りです。

- `usize`: 配列やスライスの長さ、添字によく使う符号なし整数
- `u8`: 1 バイト整数
- `[u8; 8]`: 要素数が固定の配列
- `type CompactReport = ...`: 型エイリアス

Rust では、配列とベクタは別物です。

- `[u8; 8]` は長さ固定
- `Vec<u8>` は可変長
- `&[u8]` は「配列やベクタの一部を参照するスライス」

### スライス

`&[u8]` は Rust で非常によく使います。これは「所有しない読み取り専用のビュー」です。

出典: `src/serial/mod.rs`

```rust
pub fn push_chunk(&mut self, port: &str, bytes: &[u8]) -> Vec<Vec<u8>> {
    let pending = self.pending.entry(String::from(port)).or_default();
    pending.extend_from_slice(bytes);
    take_complete_lines(pending)
}
```

この関数は `bytes` を所有していません。呼び出し元が持っているバイト列を「借りて読んでいる」だけです。C で言えば `const uint8_t*` と長さの組に近いですが、Rust のスライスは長さ情報を一緒に持っているので、かなり安全です。

### 文字列型

Rust の文字列には代表的に 2 種類あります。

- `String`: 所有する伸縮可能な UTF-8 文字列
- `&str`: 文字列スライス

`acs` のコードでも両方が頻繁に出てきます。

出典: `src/input/ds4_hid.rs`

```rust
pub struct Ds4DeviceInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: i32,
    pub product_name: Option<String>,
    pub transport: &'static str,
}
```

- `path` は実行時に生成されるので `String`
- `transport` は `"usb"` や `"bluetooth"` のような固定文字列なので `&'static str`

### 標準ライブラリのコレクション

このプロジェクトでは `Vec` のほかに `BTreeMap` と `VecDeque` が使われています。

出典: `src/ui/text_dashboard.rs`

```rust
use std::collections::{BTreeMap, VecDeque};

struct Section {
    kind: SectionKind,
    port: String,
    status: String,
    baud_rate: Option<u32>,
    display_mode: PortDisplayMode,
    entries: VecDeque<Entry>,
    rate_samples: VecDeque<RateSample>,
}
```

- `Vec<T>`: 伸縮可能な配列
- `VecDeque<T>`: 両端キュー
- `BTreeMap<K, V>`: キー順に保持される連想配列

Python の `list` と `dict` だけで済ませる感覚より、用途ごとに型を選ぶ文化が強いと考えるとよいです。

---

## 第4章 構造体、列挙型、パターンマッチ

Rust は「データの形」を明確に定義する言語です。その中心になるのが構造体と列挙型です。

### 構造体

出典: `src/serial/mod.rs`

```rust
#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub port: String,
    pub baud_rate: u32,
}
```

構造体は C の `struct` にかなり近いです。ただし Rust では、構造体に対して `impl` ブロックでメソッドを生やせるので、Python のクラスに近い使い方もできます。

`#[derive(Debug, Clone)]` は自動実装です。

- `Debug`: `{:?}` で表示できる
- `Clone`: 明示的な複製ができる

### 列挙型

Rust の enum は非常に強力です。単なる整数定数ではなく、「場合によって中身を持てる型」です。

出典: `src/serial/mod.rs`

```rust
#[derive(Debug)]
pub enum SerialError {
    List(SerialPortLibError),
    Open {
        port: String,
        source: SerialPortLibError,
    },
    Configure {
        port: String,
        source: SerialPortLibError,
    },
    NoSerialPortFound,
    MultiplePortsFound(usize),
    PortNotFound(String),
}
```

この 1 つの enum の中に、次の 3 種類のバリアントが共存しています。

- ユニット風: `NoSerialPortFound`
- タプル風: `MultiplePortsFound(usize)`
- 構造体風: `Open { port: String, source: ... }`

これが Rust の強さです。C なら enum と union と struct を組み合わせて表現したくなるような内容を、1 つの型で安全に表せます。

### パターンマッチ

enum を使うと `match` が真価を発揮します。

出典: `src/input/compact.rs`

```rust
impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReportTooShort { expected, actual } => {
                write!(
                    f,
                    "report too short: expected at least {expected} bytes, got {actual}"
                )
            }
            Self::InvalidDpad(value) => write!(f, "invalid d-pad value: {value}"),
        }
    }
}
```

`match self` で enum の種類ごとに分岐し、その中身を取り出しています。Python の `isinstance` と属性アクセスを 1 行にまとめたような感覚です。

### スライスパターン

Rust のパターンマッチは配列やスライスにも使えます。

出典: `src/input/ds4_hid.rs`

```rust
match devices.as_slice() {
    [] => Err(Ds4Error::DeviceNotFound),
    [device] => Ok(*device),
    many => Err(Ds4Error::MultipleDevicesFound(many.len())),
}
```

これはとても Rust らしいコードです。

- 空なら `[]`
- 1 要素なら `[device]`
- それ以外は `many`

C だと `if (len == 0) ... else if (len == 1) ...` と書くところを、もっと宣言的に書けています。

---

## 第5章 `Option` と `Result` とエラー処理

Rust には例外機構がありません。代わりに、失敗や未設定を値として表現します。中心になるのが `Option<T>` と `Result<T, E>` です。

### `Option<T>`

「値があるかもしれないし、無いかもしれない」という場合に使います。

参考例:

```rust
fn optional_path(
    object: &JsonObject,
    key: &str,
    base_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(value) = optional_string(object, key)? else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Ok(Some(path));
    }
    Ok(Some(base_dir.join(path)))
}
```

この関数はとても勉強になります。

- `optional_string(...)` は `Result<Option<String>, String>` を返す
- `?` で `Result` のエラーを先に処理する
- `let Some(value) = ... else { ... };` で `Option` を分解する

Python なら `None` を返し、C ならヌルポインタやフラグで表しそうなところを、型で明確に区別しています。

### `Result<T, E>`

失敗の理由も返したいときは `Result<T, E>` を使います。

出典: `src/output/formats/mod.rs`

```rust
pub fn parse(value: &str) -> Result<Self, String> {
    OUTPUT_FORMATS
        .iter()
        .find(|definition| {
            definition
                .names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(value))
        })
        .map(|definition| definition.format)
        .ok_or_else(|| format!("unsupported output format: {value}"))
}
```

このコードの流れは次の通りです。

1. `iter()` で一覧を順に見る
2. `find(...)` で該当要素を探す
3. 見つかれば `Some(...)`
4. 見つからなければ `None`
5. `ok_or_else(...)` で `Option` を `Result` に変換する

### `?` 演算子

Rust では `?` が非常に重要です。エラーがあったらその場で呼び出し元に返し、成功したら中身だけ取り出します。

出典: `src/input/compact.rs`

```rust
pub fn convert_input_report(report: &[u8]) -> Result<CompactReport, CompactError> {
    let hid10 = normalize_input_report(report)?;
    convert_hid10(&hid10)
}
```

出典: `build.rs`

```rust
fn git_branch() -> Option<String> {
    let branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        Some(String::from("detached"))
    } else {
        Some(branch)
    }
}
```

`?` は `Result` だけでなく `Option` にも使える、という点も覚えておくと便利です。

### `map_err` と `unwrap_or_else`

Rust では、エラーや値の変換もメソッドチェーンで書くことが多いです。

出典: `src/serial/mod.rs`

```rust
reader
    .set_timeout(Duration::from_millis(READ_TIMEOUT_MILLIS))
    .map_err(|source| SerialError::Configure {
        port: port_name.clone(),
        source,
    })?;
```

`map_err` は「失敗したときのエラー値だけを変換する」メソッドです。

出典: `build.rs`

```rust
let commit = env_or_git("ACS_BUILD_COMMIT", &["rev-parse", "HEAD"])
    .unwrap_or_else(|| UNKNOWN.to_owned());
```

`unwrap_or_else` は「値が無いときだけクロージャを実行して代替値を作る」メソッドです。

---

## 第6章 所有権、借用、ミュータブル参照

Rust 最大の特徴が所有権です。最初は難しく見えますが、要点は単純です。

- 各値には所有者がいる
- 同時に複数の所有者は基本的にいない
- 値を読みたいだけなら借用する
- 書き換えたいなら排他的な可変借用が必要

### 所有する型と借りる型

出典: `src/serial/mod.rs`

```rust
pub type SerialCallback = Arc<dyn Fn(SerialEvent) + Send + Sync>;

pub struct SerialInputLine {
    pub port: String,
    pub bytes: Vec<u8>,
}

impl SerialLineBuffer {
    pub fn push_chunk(&mut self, port: &str, bytes: &[u8]) -> Vec<Vec<u8>> {
        let pending = self.pending.entry(String::from(port)).or_default();
        pending.extend_from_slice(bytes);
        take_complete_lines(pending)
    }
}
```

ここでは次の対比が見えます。

- `String`, `Vec<u8>` は所有する型
- `&str`, `&[u8]` は借りる型
- `&mut self` は「このオブジェクトを可変で借りる」

### なぜ `Vec<u8>` を返すのか

所有権が必要な理由は、ローカル変数の寿命を考えると分かります。

出典: `src/input/ds4_hid.rs`

```rust
fn read_next_report_with_timeout(
    device: &HidDevice,
    timeout_millis: i32,
) -> Result<Option<Vec<u8>>, Ds4Error> {
    let mut buffer = [0u8; MAX_REPORT_SIZE];
    let bytes_read = device.read_timeout(&mut buffer, timeout_millis)?;

    if bytes_read == 0 {
        return Ok(None);
    }

    Ok(Some(buffer[..bytes_read].to_vec()))
}
```

この関数の `buffer` は関数を抜けると消えます。したがって `&buffer[..bytes_read]` のような参照をそのまま返すことはできません。そこで `.to_vec()` で新しい所有物を作り、それを返しています。

これは C の「関数ローカル配列へのポインタを返してはいけない」という話に似ています。ただし Rust は、その危険をコンパイル時に禁止してくれます。

### `clone` は明示的

Rust では、重い複製は勝手には起きません。必要なら `clone()` や `to_owned()` を明示します。

出典: `src/input/ds4_hid.rs`

```rust
.ok_or_else(|| Ds4Error::DeviceSelectionNotFound(selector.to_owned()))
```

出典: `src/serial/mod.rs`

```rust
port: self.port_name.clone(),
```

この性質のおかげで、「ここでコピーが起きているのか」が読み手に分かりやすくなります。

### `std::mem::take`

所有権を安全に移すための便利な関数もあります。

出典: `src/serial/mod.rs`

```rust
lines.push(SerialInputLine {
    port: port.clone(),
    bytes: std::mem::take(bytes),
});
```

`std::mem::take(bytes)` は、`bytes` の中身を取り出して空にします。C の `swap` や move 的な操作を、所有権ルールの中で安全にやっていると考えると理解しやすいです。

### 可変借用は排他的

Rust は「読み取り専用の共有参照」は複数許しますが、「書き換え可能な可変参照」は同時に 1 つしか許しません。データ競合や破壊的な alias を避けるためです。

このルールのため、はじめは borrow checker に叱られます。しかし、その厳しさが後で大きな安心になります。

---

## 第7章 `impl`、メソッド、関連関数

Rust では、構造体や enum に対する振る舞いを `impl` ブロックで定義します。

出典: `src/output/formats/packetacv6/mod.rs`

```rust
struct PacketAcV6OutputDriver {
    encoder: PacketAcV6PacketEncoder,
    sound_player: ModeSoundPlayer,
}

impl PacketAcV6OutputDriver {
    fn new() -> Self {
        Self {
            encoder: PacketAcV6PacketEncoder::new(),
            sound_player: ModeSoundPlayer::new(),
        }
    }
}
```

`new()` は関連関数です。`self` を取らないので、インスタンスメソッドではありません。Python の `@classmethod` よりは、単なる名前空間付き関数に近いです。

### `self`, `&self`, `&mut self`

Rust のメソッドの受け取り方は大きく 3 種類あります。

- `self`: 所有権を受け取る
- `&self`: 読み取り専用で借りる
- `&mut self`: 書き換え可能で借りる

出典: `src/output/formats/packetacv6/mod.rs`

```rust
impl OutputDriver for PacketAcV6OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String> {
        let update = self.encoder.encode_compact_report_update(compact_report);
        if update.profile_changed {
            self.sound_player.play(update.profile.as_str());
        }
        Ok(update.packet.to_vec())
    }
}
```

`encode` は内部状態の `encoder` や `sound_player` を更新しうるので `&mut self` です。

### 標準トレイトの実装

Rust では標準ライブラリのトレイトを実装して、型の使い勝手を上げることが多いです。

出典: `src/input/ds4_hid.rs`

```rust
impl From<HidError> for Ds4Error {
    fn from(value: HidError) -> Self {
        Self::Hid(value)
    }
}
```

これで `HidError` を `Ds4Error` に変換できるようになります。

出典: `src/serial/mod.rs`

```rust
impl Drop for SerialMonitor {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}
```

`Drop` はデストラクタに相当します。C++ の RAII にかなり近い考え方です。Python の `__del__` より、実行タイミングがはるかに明確です。

---

## 第8章 トレイトと trait object

Rust のトレイトは「この型はこの振る舞いを持つ」という約束です。C の関数ポインタ表や、Java の interface、Python の抽象基底クラスに少し似ています。

### トレイトの定義

出典: `src/output/formats/mod.rs`

```rust
pub trait OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String>;
}
```

このトレイトを実装した型なら、compact report を何らかの出力バイト列へ変換できます。

### trait object

出典: `src/output/formats/mod.rs`

```rust
struct OutputFormatDefinition {
    format: OutputFormat,
    names: &'static [&'static str],
    create_driver: Option<fn() -> Box<dyn OutputDriver>>,
    encode_dummy_payload: fn() -> Result<Vec<u8>, String>,
}
```

`Box<dyn OutputDriver>` は trait object です。「`OutputDriver` を実装した何か 1 つ」をヒープに置いて扱います。具体型を隠したいときに使います。

出典: `src/pipeline/engine.rs`

```rust
struct PipelineInstance {
    inputs: Vec<String>,
    filter: Box<dyn FrameFilter>,
    transforms: Vec<Box<dyn MessageTransform>>,
    classifier: Box<dyn MessageClassifier>,
    router: Box<dyn MessageRouter>,
}
```

この設計により、`PipelineEngine` は「どの具体型のフィルタか」を知らなくても処理できます。柔軟性が高く、プラグイン的な構造を作りやすくなります。

### トレイト実装の組み立て

出典: `src/pipeline/modules/mod.rs`

```rust
pub(crate) fn build_filter(config: &FilterModuleConfig) -> Box<dyn FrameFilter> {
    match config {
        FilterModuleConfig::AllowAll => Box::new(AllowAllFilter),
        FilterModuleConfig::DropEmpty => Box::new(DropEmptyFilter),
        FilterModuleConfig::MatchSource { input_ids } => Box::new(MatchSourceFilter {
            input_ids: input_ids.clone(),
        }),
        FilterModuleConfig::MatchPrefix { prefix } => Box::new(MatchPrefixFilter {
            prefix: prefix.clone(),
        }),
    }
}
```

これは Rust らしい「設定 enum から具体実装を選ぶ」書き方です。C でやると関数ポインタ群になりがちで、Python でやるとクラス辞書になりがちな部分を、型安全に書けています。

---

## 第9章 ジェネリクス、クロージャ、イテレータ

Rust の強力さは、抽象化しても実行時コストを増やしにくいところにあります。その中心がジェネリクスとイテレータです。

### ジェネリクス

出典: `src/session/runtime.rs`

```rust
pub(crate) fn run_loop<F, G>(
    &mut self,
    wait_interval: Duration,
    stop_requested: G,
    on_frame: F,
) -> Result<PathBuf, String>
where
    F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
    G: FnMut() -> bool,
{
    self.run_loop_with_tick(wait_interval, stop_requested, on_frame, |_| Ok(()))
}
```

この関数は、`F` と `G` という型パラメータを受け取っています。つまり、どんな closure を渡すかを呼び出し側に任せています。

ここで大事なのは、`FnMut` というトレイト境界です。

- `Fn`: 状態を変えずに呼べる
- `FnMut`: 呼び出しのたびに内部状態を変えてよい
- `FnOnce`: 1 回だけ呼べる

### `impl Into<String>`

Rust では、短いジェネリクスを書くために `impl Trait` もよく使います。

出典: `src/ui/text_dashboard.rs`

```rust
pub fn new(title: impl Into<String>) -> io::Result<Self> {
    let mut stdout = io::stdout();
    // ...
    Ok(Self {
        title: title.into(),
        header_lines: Vec::new(),
        sections: BTreeMap::new(),
        previous_lines: Vec::new(),
        stdout,
    })
}
```

これにより、呼び出し側は `String` でも `&str` でも渡せます。Python 的には柔軟で、C よりずっと型安全です。

### イテレータとメソッドチェーン

出典: `src/output/formats/mod.rs`

```rust
OUTPUT_FORMATS
    .iter()
    .find(|definition| {
        definition
            .names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(value))
    })
    .map(|definition| definition.format)
    .ok_or_else(|| format!("unsupported output format: {value}"))
```

これは Rust のイテレータ文化をよく表しています。

- `iter()` で反復子を作る
- `find(...)` で条件に合う 1 件を探す
- `map(...)` で中身を変換する
- `ok_or_else(...)` で `Result` にする

Python のリスト内包表記や `map/filter`、C++ の ranges に近い感覚です。

### `collect::<Vec<_>>()`

出典: `src/pipeline/engine.rs`

```rust
let pipelines = spec
    .pipelines
    .iter()
    .map(|definition| {
        Ok(PipelineInstance {
            inputs: definition.inputs.clone(),
            filter: build_filter(&definition.filter),
            transforms: build_transform_chain(&definition.transform, &definition.inputs)?,
            classifier: build_classifier(&definition.classify),
            router: build_router(&definition.router),
        })
    })
    .collect::<Result<Vec<_>, String>>()?;
```

`Vec<_>` の `_` は型推論です。全部書くと冗長になるので、コンパイラに任せています。Rust では「必要なら書く、不要なら省く」のバランスがかなり良いです。

---

## 第10章 ライフタイム

Rust 学習で最も身構えやすいのがライフタイムです。しかし、実際には「参照がどの入力に依存しているかを明示する記法」にすぎません。

### まずは直感

ライフタイムはメモリ管理機構そのものではありません。メモリ管理は所有権が行い、ライフタイムは「この参照は少なくともこれくらい生きていなければならない」という関係を型に刻みます。

### 明示ライフタイムの典型例

参考例:

```rust
fn expect_object<'a>(value: &'a JsonValue, name: &str) -> Result<&'a JsonObject, String> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("`{name}` must be a JSON object")),
    }
}
```

この `'a` は、「返している `&JsonObject` は、引数 `value` の中身を借りたものです」という意味です。

もし `'a` が無いと、コンパイラは「返り値の参照がどこから来たのか」を判断できません。

### もう一つの例

出典: `src/input/ds4_hid.rs`

```rust
fn select_device_info<'a>(
    api: &'a HidApi,
    selector: Option<&str>,
) -> Result<&'a hidapi::DeviceInfo, Ds4Error> {
    let devices = api
        .device_list()
        .filter(|device| is_dualshock_4(device.vendor_id(), device.product_id()))
        .collect::<Vec<_>>();

    // 省略
}
```

ここでも「返り値の `DeviceInfo` 参照は `api` にひも付く」と表現しています。

### `'static`

出典: `src/input/ds4_hid.rs`

```rust
pub struct Ds4DeviceInfo {
    // ...
    pub transport: &'static str,
}
```

`'static` は「プログラム全体のあいだ生きる参照」です。文字列リテラル `"usb"` や `"bluetooth"` は実行ファイル中に固定で置かれるので、`&'static str` にできます。

出典: `src/output/formats/mod.rs`

```rust
names: &'static [&'static str],
```

ここでも同様に、フォーマット名の配列を静的データとして持っています。

### 大事な理解

ライフタイム注釈は寿命を延ばしません。あくまで「この参照はこの入力より長くは生きられない」と説明するだけです。

この章は一度で完璧に分からなくても大丈夫です。まずは次の 2 点を覚えてください。

- 所有権がある値ならライフタイムをあまり気にしなくてよい
- 参照を返す関数で、返り値がどの引数に依存するかを示すのがライフタイム

---

## 第11章 並行処理と共有状態

`acs` はシリアル監視や UI 更新を同時進行で行うので、Rust の並行処理の良い実例になっています。

### スレッド起動

出典: `src/serial/mod.rs`

```rust
fn spawn_reader_thread(
    port_name: String,
    mut reader: Box<dyn SerialPort>,
    stop_requested: Arc<AtomicBool>,
    callback: SerialCallback,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER_SIZE];
        // 省略
    })
}
```

ここでは `thread::spawn(move || { ... })` を使っています。

- `move` は、クロージャが外側の値の所有権を受け取ることを意味します
- スレッドの中で使う値は、元の関数が終わっても安全に生きていなければならないので、`move` がよく必要になります

### `Arc` と `AtomicBool`

出典: `src/serial/mod.rs`

```rust
pub struct SerialMonitor {
    stop_requested: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
}
```

- `Arc<T>` はスレッド安全な参照カウントポインタです
- `AtomicBool` はロックなしで安全に読み書きできる真偽値です

Python の参照共有はかなり気軽ですが、Rust では「共有してよい型か」「競合しないか」を型システムで厳しく見ます。

### チャネル

出典: `src/session/runtime.rs`

```rust
let (event_tx, event_rx) = mpsc::channel::<SessionEvent>();
```

reader thread からの入力イベントは、このチャネルを通じてメインスレッドへ送られます。

出典: `src/session/runtime.rs`

```rust
match self.event_rx.recv_timeout(wait_interval) {
    Ok(event) => self.handle_event(event, on_frame)?,
    Err(mpsc::RecvTimeoutError::Timeout) => {}
    Err(mpsc::RecvTimeoutError::Disconnected) => {
        if !self.inputs_disconnected {
            self.dashboard.set_status("input disconnected");
            self.dirty = true;
            self.inputs_disconnected = true;
        }
        thread::sleep(wait_interval);
        return Ok(());
    }
}
```

この設計の良いところは、スレッド間で複雑な共有可変状態を持たなくてよいことです。メッセージ渡しにすると、競合の原因をかなり減らせます。

### `Send` と `Sync`

出典: `src/serial/mod.rs`

```rust
pub type SerialCallback = Arc<dyn Fn(SerialEvent) + Send + Sync>;
```

ここで付いている `Send` と `Sync` は、スレッド境界を越えて扱ってよいことを示すトレイトです。Rust の並行処理が安全なのは、これらの制約をコンパイル時に確認しているからです。

---

## 第12章 `unsafe`、FFI、条件付きコンパイル

Rust は安全な言語ですが、OS や C ライブラリとやり取りするときは `unsafe` が必要です。このプロジェクトにもその例があります。

### `unsafe` は「危険な場所を狭く囲う」ための印

出典: `src/common.rs`

```rust
let now = unsafe { libc::time(std::ptr::null_mut()) };
```

出典: `src/common.rs`

```rust
let result = unsafe { libc::localtime_r(&now, local_time.as_mut_ptr()) };
```

ここで unsafe が必要なのは、C 関数呼び出しの正しさをコンパイラが保証できないからです。ただし、unsafe を書いたからといってプログラム全体が unsafe になるわけではありません。この狭い範囲だけ、人間が責任を持って正しさを保証する、という意味です。

### 外部関数宣言

出典: `src/ui/text_dashboard.rs`

```rust
unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}
```

これは C の関数宣言を Rust から見えるようにしたものです。FFI と呼ばれます。

### 条件付きコンパイル

Rust は OS ごとの差分も `cfg` で整理できます。

出典: `src/common.rs`

```rust
#[cfg(unix)]
fn local_timestamp(format: &str) -> String {
    // Unix 実装
}

#[cfg(not(unix))]
fn local_timestamp(_: &str) -> String {
    unix_fallback()
}
```

出典: `src/ui/text_dashboard.rs`

```rust
#[cfg(unix)]
use std::os::fd::AsRawFd;
```

このように書くと、Unix 環境でだけそのコードがコンパイルされます。C の `#ifdef` に近いですが、Rust では属性として書くので構造が見やすいです。

---

## 第13章 テストとビルドスクリプト

Rust は標準でテスト機構を持っています。`acs` でも多くのファイルにユニットテストがあります。

### ユニットテスト

出典: `src/input/compact.rs`

```rust
#[cfg(test)]
mod tests {
    use super::{CompactReport, convert_input_report};

    #[test]
    fn converts_bluetooth_example_from_requirements() {
        let report = [0x11, 255, 128, 1, 127, 7, 0x52, 0x03, 255, 64];

        assert_eq!(
            convert_input_report(&report).unwrap(),
            [0x09, 0xD6, 0xFF, 0x80, 0x01, 0x7F, 0xFF, 0x40]
        );
    }
}
```

`#[cfg(test)]` はテストビルド時だけ有効、`#[test]` はテスト関数です。Python の `pytest` より標準装備感が強く、C よりずっと手軽です。

### ビルドスクリプト

`build.rs` はビルド前に走る特別な Rust プログラムです。

出典: `build.rs`

```rust
fn main() {
    for key in [
        "ACS_BUILD_BRANCH",
        "ACS_BUILD_COMMIT",
        "ACS_BUILD_DIRTY",
        "ACS_BUILD_SOURCE_KIND",
        "ACS_BUILD_SOURCE_REF",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    println!("cargo:rustc-env=ACS_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=ACS_BUILD_BRANCH={branch}");
}
```

このスクリプトは、Git の commit や branch 情報を埋め込む役割を持っています。Rust ではビルド周辺も Rust で書けるため、統一感があります。

### `env!` マクロ

ビルド時に埋め込んだ値は `env!` で参照できます。たとえば `src/app/cli/version.rs` では、ビルドメタデータをバージョン表示に使っています。

---

## 第14章 まとめと次の学習

このプロジェクトを通して、Rust の主要な要素をひと通り見てきました。

- モジュールと crate
- 変数、関数、式、`match`
- 配列、スライス、`Vec`、`String`
- 構造体、enum、パターンマッチ
- `Option`、`Result`、`?`
- 所有権、借用、可変参照
- `impl`、関連関数、`Drop`
- トレイト、trait object
- ジェネリクス、クロージャ、イテレータ
- ライフタイム
- スレッド、チャネル、`Arc`
- `unsafe`、FFI、`cfg`
- テスト、ビルドスクリプト

もし今の段階でまだ不安が残っているなら、それは自然です。Rust は「一度読んで終わり」の言語ではなく、実コードを何度も読み返しながら感覚を育てる言語です。

### 次におすすめの読み方

1. `src/input/compact.rs` を読み、配列、スライス、`Result`、`match` を復習する
2. `src/output/formats/mod.rs` と `src/output/formats/packetacv6/mod.rs` を読み、enum と trait を確認する
3. `src/pipeline/engine.rs` と `src/pipeline/modules/mod.rs` を読み、trait object とイテレータに慣れる
4. `src/session/runtime.rs` と `src/serial/mod.rs` を読み、所有権と並行処理を意識する
5. `src/app/cli/send.rs` や `src/app/cli/route.rs` を読み、`Option`、`Result`、ライフタイム、引数解析を追う

### 最後に

Rust の学習では、「文法を覚える」ことよりも「この値は誰が所有しているのか」「今は借りているだけなのか」「失敗は型で表せているか」を考える習慣が大切です。

この `acs` のコードは、その感覚を養うための良い教材になっています。分からなくなったら抽象論に戻るのではなく、もう一度このプロジェクトの具体的なコードに戻ってきてください。Rust は、具体例から学ぶほど強くなる言語です。
