use std::process::ExitCode;

pub(crate) fn print_usage(bin_name: &str) {
    eprintln!("使い方: {bin_name} <コマンド>");
    eprintln!();
    eprintln!("コマンド:");
    eprintln!("  control     DUALSHOCK 4 の入力を読み取り、シリアル出力へ送信");
    eprintln!("  io          複数シリアルポートの送信と受信を同一画面で監視");
    eprintln!("  monitor     1 つ以上のシリアルポートを監視");
    eprintln!("  route       シリアル入力と出力の間でバイト列を中継");
    eprintln!("  send        ダミーペイロードをシリアルポートへ繰り返し送信");
    eprintln!("  xbee-mock   xbee-test の通信モデルの片側を単一ポートで実行");
    eprintln!("  xbee-rtt    1 組の XBee 間で接続性と往復時間を確認");
    eprintln!("  xbee-test   base/remote ポート間で AU/RU と AD/RD を相互試験");
    eprintln!("  controllers 接続中の DUALSHOCK 4 コントローラーを一覧表示");
    eprintln!("  ports       利用可能なシリアルポートを一覧表示");
    eprintln!("  version     ビルド版情報とソース情報を表示");
    eprintln!("  help        コマンドのヘルプを表示");
    eprintln!();
    eprintln!(
        "インストール済みビルドの情報は `{bin_name} --version` または `{bin_name} version` で確認できます。"
    );
    eprintln!();
    eprintln!(
        "詳細は `{bin_name} help control`、`{bin_name} help io`、`{bin_name} help monitor`、`{bin_name} help route`、`{bin_name} help send`、`{bin_name} help xbee-mock`、`{bin_name} help xbee-rtt`、`{bin_name} help xbee-test` で確認できます。"
    );
}

pub(crate) fn print_help(bin_name: &str) {
    println!("使い方: {bin_name} <コマンド>");
    println!();
    println!("コマンド:");
    println!("  control     DUALSHOCK 4 の入力を読み取り、シリアル出力へ送信");
    println!("  io          複数シリアルポートの送信と受信を同一画面で監視");
    println!("  monitor     1 つ以上のシリアルポートを監視");
    println!("  route       シリアル入力と出力の間でバイト列を中継");
    println!("  send        ダミーペイロードをシリアルポートへ繰り返し送信");
    println!("  xbee-mock   xbee-test の通信モデルの片側を単一ポートで実行");
    println!("  xbee-rtt    1 組の XBee 間で接続性と往復時間を確認");
    println!("  xbee-test   base/remote ポート間で AU/RU と AD/RD を相互試験");
    println!("  controllers 接続中の DUALSHOCK 4 コントローラーを一覧表示");
    println!("  ports       利用可能なシリアルポートを一覧表示");
    println!("  version     ビルド版情報とソース情報を表示");
    println!("  help        コマンドのヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --config FORMAT=PacketACv6");
    println!(
        "  {bin_name} control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8 --config FORMAT=PacketACv6"
    );
    println!("  {bin_name} monitor --port /dev/ttyUSB0,utf8+packet --port /dev/ttyUSB1,hex");
    println!("  {bin_name} route merge -i in_a=/dev/ttyUSB0,utf8 -o out_main=/dev/ttyUSB1,hex");
    println!("  {bin_name} send --port /dev/ttyUSB0,hex --config FORMAT=PacketACv6");
    println!(
        "  {bin_name} send -o main=/dev/ttyUSB0@921600,hex,packetacv6 -o sub=/dev/ttyUSB1@115200,utf8,packetjfv1"
    );
    println!(
        "  {bin_name} io -i /dev/ttyUSB1@115200,utf8,packetjfv1 -o main=/dev/ttyUSB0@921600,hex,packetacv6,10"
    );
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketJFv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverUpGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverDownGeneral");
    println!("  {bin_name} xbee-rtt --port /dev/ttyUSB0");
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0 --port remote=/dev/ttyUSB1 --config AU_RATE=100,AD_RATE=100"
    );
    println!(
        "  {bin_name} xbee-mock base -p /dev/ttyUSB0 --config PAIR=1,TX_FORMAT=packetacv6@100+roverupgeneral@20,RX_FORMAT=packetjfv1+roverdowngeneral,TRAFFIC_PATTERN=flood"
    );
    println!("  {bin_name} --version");
    println!();
    println!("コントローラーまたはシリアルポートが 1 つだけ利用可能な場合は、自動で選択されます。");
    println!("各ポート指定で baud を省略した場合は 115200 が使われます。");
    println!("真偽値以外の多くの設定は `--config KEY=VALUE,...` でまとめて指定できます。");
}

pub(crate) fn print_help_topic(bin_name: &str, topic: Option<&str>) -> ExitCode {
    match topic {
        None => {
            print_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("control") => {
            print_control_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("io") => {
            print_io_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("monitor") => {
            print_monitor_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("route") => {
            print_route_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("send") => {
            print_send_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("xbee-mock") => {
            print_xbee_mock_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("xbee-rtt") => {
            print_xbee_rtt_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("xbee-test") => {
            print_xbee_test_help(bin_name);
            ExitCode::SUCCESS
        }
        Some("controllers") => {
            println!("使い方: {bin_name} controllers");
            println!();
            println!(
                "接続中の DUALSHOCK 4 コントローラーと、その接続方式、VID/PID、インターフェース、製品名、パスを一覧表示します。"
            );
            ExitCode::SUCCESS
        }
        Some("ports") => {
            println!("使い方: {bin_name} ports");
            println!();
            println!(
                "利用可能なシリアルポートと、取得できる場合は USB メタデータを一覧表示します。"
            );
            ExitCode::SUCCESS
        }
        Some("version") => {
            println!("使い方: {bin_name} --version");
            println!("       {bin_name} version");
            println!();
            println!(
                "パッケージ版に加えて、コミット、ブランチ、取得元種別、ワークツリーが dirty/clean かといったビルド元情報を表示します。"
            );
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("不明なヘルプトピックです: {other}");
            print_help(bin_name);
            ExitCode::from(2)
        }
    }
}

pub(crate) fn print_control_help(bin_name: &str) {
    println!("使い方: {bin_name} control [オプション]");
    println!();
    println!(
        "DUALSHOCK 4 コントローラーを読み取り、整形したバイト列をシリアルポートへ書き込みます。"
    );
    println!("出力ポートも入力として監視され、追加ポートは `--monitor` で増やせます。");
    println!();
    println!("オプション:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> シリアル出力ポート");
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!(
        "                              省略すると USB シリアルまたは ST-LINK が 1 つだけある場合に自動選択します"
    );
    println!("  -m, --monitor <PORT[@BAUD][,DISPLAY]> 追加で監視するシリアルポート");
    println!(
        "      --config <K=V,...>     設定をまとめて指定: `CONTROLLER`, `FORMAT`, `DISPLAY`, `LOG_DIR`"
    );
    println!("                              `FORMAT`: 出力フォーマット（現状: packetacv6）");
    println!("                              `DISPLAY`: ポートの表示モード");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!(
        "                                    monitor の入力では任意で +line/+packet を追加できます"
    );
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --config FORMAT=PacketACv6");
    println!(
        "  {bin_name} control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8 --config FORMAT=PacketACv6"
    );
    println!(
        "  {bin_name} control --config DISPLAY=input:default=utf8+packet --monitor /dev/ttyUSB1"
    );
    println!(
        "  {bin_name} control --config DISPLAY=input:/dev/ttyUSB0=utf8 --config DISPLAY=output:/dev/ttyUSB0=hex"
    );
    println!("  {bin_name} control --config CONTROLLER=0 --monitor /dev/ttyUSB1 --no-log");
}

pub(crate) fn print_send_help(bin_name: &str) {
    println!("使い方: {bin_name} send [オプション]");
    println!();
    println!("選択したフォーマットのダミーペイロードをシリアルポートへ繰り返し送信します。");
    println!("`--interactive` を付けると、端末入力を受け取り Enter ごとに 1 行送信します。");
    println!(
        "送信したパケットは `monitor` と同様にリアルタイム表示されます。Ctrl-C で終了します。"
    );
    println!("既定の送信レートは 50 Hz で、`control` の 20 ms 間隔に対応します。");
    println!(
        "複数フォーマットが混在する既知の入力ポートはフォーマットごとにデコードされ、各フォーマットの RX Hz がヘッダーに表示されます。"
    );
    println!();
    println!("オプション:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> シリアル出力ポート");
    println!(
        "  -o, --output-port <ID=PORT[@BAUD][,DISPLAY][,FORMAT][,RATE]> 追加で指定できるシリアル出力（繰り返し指定可）"
    );
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!(
        "      --config <K=V,...>     設定をまとめて指定: `FORMAT`, `RATE`, `DISPLAY`, `LOG_DIR`"
    );
    println!("                              `RATE` は `--interactive` では無視されます");
    println!(
        "                              `DISPLAY` は TARGET=MODE 形式です。例: `output:default=hex`"
    );
    println!(
        "  -i, --interactive          端末から 1 行ずつ読み取り、Enter ごとに送信（生 UTF-8 + \\r\\n）"
    );
    println!(
        "  -m, --monitor <PORT[@BAUD][,DISPLAY][,FORMAT[+FORMAT...]]> 追加で監視するシリアルポート"
    );
    println!(
        "                              FORMAT 省略時は改行待ちせず raw 入力を画面幅で折り返し表示します"
    );
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketACv6");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketMv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketIv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketBv1");
    println!("  {bin_name} send --port /dev/ttyUSB0@921600 --config FORMAT=PacketACv6,RATE=100");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketJFv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverUpGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverDownGeneral");
    println!(
        "  {bin_name} send --port /dev/ttyUSB0,hex --monitor /dev/ttyUSB1,utf8,packetjfv1 --config DISPLAY=output:default=hex"
    );
    println!(
        "  {bin_name} send --port /dev/ttyUSB0 --monitor /dev/ttyUSB1,packetmv1+packetiv1+packetbv1 --config FORMAT=PacketMv1"
    );
    println!("  {bin_name} send --port /dev/ttyUSB0 --monitor /dev/ttyUSB1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketACv6 --no-log");
    println!(
        "  {bin_name} send -o main=/dev/ttyUSB0@921600,hex,packetacv6 -o sub=/dev/ttyUSB1@115200,utf8+packet,packetjfv1"
    );
    println!(
        "  {bin_name} send -o ac=/dev/ttyUSB0@921600,hex,packetacv6,100 -o up=/dev/ttyUSB0@921600,utf8,roverupgeneral,10"
    );
    println!("  {bin_name} send --interactive --port /dev/ttyUSB0");
    println!("  {bin_name} send -i --port /dev/ttyUSB0 --monitor /dev/ttyUSB1");
    println!("  {bin_name} send --display output:default=hex --display input:default=utf8+packet");
}

pub(crate) fn print_io_help(bin_name: &str) {
    println!(
        "使い方: {bin_name} io -i [PORT[@BAUD][,DISPLAY][,FORMAT[+FORMAT...]]] -o [ID=PORT[@BAUD][,DISPLAY][,FORMAT][,RATE]]"
    );
    println!();
    println!("複数の受信ポートと送信ポートを同一画面で監視します。");
    println!(
        "送信ポートは選択したフォーマットのダミーペイロードを繰り返し送信し、受信側は raw または指定フォーマットで表示します。"
    );
    println!(
        "`-i` または `-o` を値なしで指定すると、矢印上下と Enter でポート、フォーマット、ボーレート、送信レートを選択できます。"
    );
    println!("既定値は baud=115200、送信レート=10 Hz です。");
    println!();
    println!("オプション:");
    println!("  -i, --input <PORT[@BAUD][,DISPLAY][,FORMAT[+FORMAT...]]>");
    println!(
        "                              受信ポート（繰り返し指定可）。値を省略すると対話式に選択"
    );
    println!("  -o, --output <ID=PORT[@BAUD][,DISPLAY][,FORMAT][,RATE]>");
    println!(
        "                              送信ポート（繰り返し指定可）。値を省略すると対話式に選択"
    );
    println!("                              ID= は同一ポートへ複数形式を送る場合に指定します");
    println!("                              DISPLAY: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                              受信では +line/+packet/+wrap も指定できます");
    println!(
        "                              FORMAT: packetacv6/packetmv1/packetiv1/packetbv1/packetjfv1/roverupgeneral/roverdowngeneral"
    );
    println!("      --display <TARGET=MODE> 既存コマンドと同じ表示上書き");
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} io -i");
    println!("  {bin_name} io -o");
    println!("  {bin_name} io -i /dev/ttyUSB1@115200,utf8,packetjfv1");
    println!(
        "  {bin_name} io -o main=/dev/ttyUSB0@921600,hex,packetacv6,10 -o sub=/dev/ttyUSB1@115200,utf8,roverupgeneral,10"
    );
    println!(
        "  {bin_name} io -i /dev/ttyUSB1,packetacv6+packetjfv1 -o ac=/dev/ttyUSB0@921600,hex,packetacv6,100"
    );
}

pub(crate) fn print_xbee_test_help(bin_name: &str) {
    println!("使い方: {bin_name} xbee-test [オプション]");
    println!();
    println!(
        "`base` から `remote` へ AU(PacketACv6) + RU(RoverUpGeneral)、`remote` から `base` へ AD(PacketJFv1) + RD(RoverDownGeneral) を送信します。"
    );
    println!("モードは `flood`、`ping-pong`、`polling` です。");
    println!(
        "各ポートを同時に監視し、ダッシュボードにはポートごとの TX/RX パケットレートと一致数/エラー数を表示します。"
    );
    println!("高レート監視を軽量に行うため、入出力表示は hex packet に固定されています。");
    println!(
        "ヘッダーには実際の表示 FPS も表示され、RX レートやエラーカウンタは端末更新速度とは独立して追跡されます。"
    );
    println!();
    println!("オプション:");
    println!("  -p, --port <ID=PORT[@BAUD]> ポート割り当て。ID: `base`, `remote`");
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!(
        "      --config <K=V,...>     設定をまとめて指定: `MODE`, `POLL_RATE`, `BASE_REAL_PERCENT`,"
    );
    println!(
        "                              `REMOTE_REAL_PERCENT`, `AU_RATE`, `RU_RATE`, `AD_RATE`, `RD_RATE`, `LOG_DIR`"
    );
    println!(
        "                              `MODE`: `flood`, `ping-pong`, `polling`（既定値: flood）"
    );
    println!("                              `AD_RATE` / `RD_RATE` は `ping-pong` では無視されます");
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} xbee-test --port base=/dev/ttyUSB0 --port remote=/dev/ttyUSB1");
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0@921600 --port remote=/dev/ttyUSB1@921600 --config AU_RATE=100,RU_RATE=50,AD_RATE=80,RD_RATE=40"
    );
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0 --port remote=/dev/ttyUSB1 --config MODE=ping-pong,AU_RATE=100,RU_RATE=50"
    );
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0 --port remote=/dev/ttyUSB1 --config MODE=polling,POLL_RATE=100,BASE_REAL_PERCENT=10,REMOTE_REAL_PERCENT=20"
    );
    println!("  {bin_name} xbee-test --port base=/dev/ttyUSB0 --port remote=/dev/ttyUSB1 --no-log");
}

pub(crate) fn print_xbee_rtt_help(bin_name: &str) {
    println!("使い方: {bin_name} xbee-rtt [オプション]");
    println!();
    println!("1 組の XBee 間で、対称な接続確認と双方向 RTT 計測を行います。");
    println!(
        "通常モードでは各 PC にローカルなシリアルポートを 1 つずつ使い、両方の PC で同じコマンドを実行すると、どちらが先に測定するかを自動交渉します。"
    );
    println!(
        "`--port` を 2 回指定した場合だけ、1 プロセスで 2 つのローカル XBee を駆動し、ローカルペアとして扱います。"
    );
    println!(
        "ノイズ後にもデコーダーが再同期できるよう、セッション ID・長さ・CRC16 を含むコンパクトなバイナリフレームを使います。"
    );
    println!();
    println!("オプション:");
    println!(
        "  -p, --port <PORT[@BAUD]>    1 台の XBee モジュールに接続するローカルシリアルポート"
    );
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!(
        "                              省略するとシリアルポートが 1 つだけある場合に自動選択します"
    );
    println!("                              1 PC ローカルペアモードのときだけ 2 回指定します");
    println!(
        "      --config <K=V,...>      設定をまとめて指定: `PAYLOAD_SIZE`, `COUNT`, `INTERVAL_MS`,"
    );
    println!("                              `PROBE_TIMEOUT_MS`, `CONNECT_TIMEOUT_MS`");
    println!("      --show-wire            送受信バイト列を hex で継続表示。TX は青、RX は赤");
    println!(
        "                            1 PC ローカルペアモードでは各チャンクに [0>] / [1<] を付けます"
    );
    println!(
        "      --show-protocol        デコード済みの HELLO / PROBE / RESULT 形式ログを継続表示"
    );
    println!("                            同じ青/赤の色分けと [0>] / [1<] 接頭辞を使います");
    println!("      --s3b                 XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} xbee-rtt --port /dev/ttyUSB0");
    println!("  # もう片方の PC でも同じコマンドを実行");
    println!("  {bin_name} xbee-rtt --port /dev/ttyUSB0 --show-wire");
    println!("  {bin_name} xbee-rtt --port /dev/ttyUSB0 --show-protocol");
    println!(
        "  {bin_name} xbee-rtt --port /dev/ttyUSB0@921600 --config PAYLOAD_SIZE=64,COUNT=20,INTERVAL_MS=50"
    );
    println!(
        "  {bin_name} xbee-rtt --port /dev/ttyUSB0@921600 --port /dev/ttyUSB1@921600 --config PAYLOAD_SIZE=64,COUNT=20,INTERVAL_MS=50"
    );
}

pub(crate) fn print_xbee_mock_help(bin_name: &str) {
    println!("使い方: {bin_name} xbee-mock <ロール> [オプション]");
    println!("       {bin_name} xbee-mock --role <ロール> [オプション]");
    println!();
    println!(
        "明示的な uplink/downlink 割り当てで、xbee 通信モデルの `base` 側または `remote` 側を実行します。"
    );
    println!(
        "`PAIR=1` では 1 つのポートを両方向で共有します。`PAIR=2` では `up` と `down` を別々に割り当てます。"
    );
    println!("高レート監視を軽量に行うため、表示は hex packet モード固定です。");
    println!();
    println!("ロール:");
    println!("  base    uplink が TX、downlink が RX");
    println!("  remote  uplink が RX、downlink が TX");
    println!();
    println!("オプション:");
    println!("  -p, --port <PORT[@BAUD]>   `PAIR=1` 用のポート割り当て");
    println!("  -p, --port <up=PORT[@BAUD]> `PAIR=2` 用の uplink 割り当て");
    println!("  -p, --port <down=PORT[@BAUD]> `PAIR=2` 用の downlink 割り当て");
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!(
        "      --config <K=V,...>     設定をまとめて指定: `ROLE`, `PAIR`, `TX_FORMAT`, `RX_FORMAT`, `TRAFFIC_PATTERN`, `LOG_DIR`"
    );
    println!("                              `PAIR`: `1` または `2`（既定値: 1）");
    println!("                              `TX_FORMAT`: `<FORMAT[@RATE]>[+<FORMAT[@RATE]>...]`");
    println!("                              `RX_FORMAT`: `<FORMAT>[+<FORMAT>...]`");
    println!("                              `TRAFFIC_PATTERN`: `flood`, `ping-pong`, `polling`");
    println!(
        "                              Poll 系フォーマットは `PollGreeting` / `PollResponse` です"
    );
    println!("                              旧エイリアス: `--option`");
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!(
        "  {bin_name} xbee-mock base -p /dev/ttyUSB0@921600 --config PAIR=1,TX_FORMAT=packetacv6@100+roverupgeneral@20,RX_FORMAT=packetjfv1+roverdowngeneral,TRAFFIC_PATTERN=flood"
    );
    println!(
        "  {bin_name} xbee-mock base -p up=/dev/ttyUSB0@921600 -p down=/dev/ttyUSB1@921600 --config PAIR=2,TX_FORMAT=packetacv6@100+roverupgeneral@20,RX_FORMAT=packetjfv1+roverdowngeneral,TRAFFIC_PATTERN=ping-pong"
    );
    println!(
        "  {bin_name} xbee-mock remote -p up=/dev/ttyUSB0@921600 -p down=/dev/ttyUSB1@921600 --config PAIR=2,TX_FORMAT=pollresponse@100,RX_FORMAT=pollgreeting,TRAFFIC_PATTERN=polling"
    );
    println!(
        "  {bin_name} xbee-mock -p /dev/ttyUSB0 --config ROLE=remote,PAIR=1,TX_FORMAT=packetjfv1@80+roverdowngeneral@70,RX_FORMAT=packetacv6+roverupgeneral,TRAFFIC_PATTERN=flood --no-log"
    );
}

pub(crate) fn print_monitor_help(bin_name: &str) {
    println!("使い方: {bin_name} monitor [オプション]");
    println!();
    println!("1 つ以上のシリアルポートを監視し、各ポートにつき最新 10 件を表示します。");
    println!();
    println!("オプション:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> 監視するシリアルポート（繰り返し指定可）");
    println!(
        "                              PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                              BAUD を省略した場合は 115200 が使われます");
    println!("      --config <K=V,...>     設定をまとめて指定: `DISPLAY`, `LOG_DIR`");
    println!("                              `DISPLAY`: ポートの表示モード");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!(
        "                                    monitor の入力では任意で +line/+packet を追加できます"
    );
    println!("      --no-log               ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                  XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                 このヘルプを表示");
    println!();
    println!("例:");
    println!("  {bin_name} monitor --port /dev/ttyUSB0");
    println!("  {bin_name} monitor --port /dev/ttyUSB0,utf8+packet --port /dev/ttyUSB1,hex");
    println!(
        "  {bin_name} monitor --config DISPLAY=input:/dev/ttyUSB0=utf8+packet --config DISPLAY=input:default=hex+utf8+line"
    );
    println!("  {bin_name} monitor --port /dev/ttyUSB0@921600 --port /dev/ttyUSB1@115200 --no-log");
}

pub(crate) fn print_route_help(bin_name: &str) {
    println!("使い方: {bin_name} route [テンプレート] [オプション]");
    println!();
    println!("1 つ以上のシリアル入力から 1 つ以上のシリアル出力へバイト列を中継します。");
    println!("中継動作は組み込みルートテンプレートから選べます。");
    println!();
    println!("オプション:");
    println!("      --list-templates        組み込みルートテンプレートを表示");
    println!("  -i, --input-port <ID=PORT[@BAUD][,DISPLAY]>  中継元の入力ポート（繰り返し指定可）");
    println!("  -o, --output-port <ID=PORT[@BAUD][,DISPLAY]> 中継先の出力ポート（繰り返し指定可）");
    println!(
        "                               PORT にはデバイスパスまたは `acs ports` の番号を指定できます"
    );
    println!("                               BAUD を省略した場合は 115200 が使われます");
    println!("      --config <K=V,...>      設定をまとめて指定: `TEMPLATE`, `DISPLAY`, `LOG_DIR`");
    println!("                              `TEMPLATE` は位置引数の TEMPLATE と同じです");
    println!("                              `DISPLAY`: ポートの表示モード");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!(
        "                                    monitor の入力では任意で +line/+packet を追加できます"
    );
    println!("      --no-log                ログファイル作成を無効化して最大スループットを優先");
    println!("      --s3b                   XBee Pro 900-HP (S3B) bootloader menu を事前復帰");
    println!(
        "                              互換性のため、値を取る旧形式フラグも引き続き利用可能です"
    );
    println!("  -h, --help                  このヘルプを表示");
    println!();
    println!("組み込みテンプレート:");
    println!("  merge            到着順のバイト列をすべての出力へそのまま転送");
    println!("  one-to-one       入出力配列を順番で対応付けてそのまま中継");
    println!();
    println!("例:");
    println!("  {bin_name} route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1");
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0@921600 -i in_b=/dev/ttyUSB1@115200 -o out_main=/dev/ttyUSB2@921600"
    );
    println!(
        "  {bin_name} route -i in_a=/dev/ttyUSB0,utf8 -o out_main=/dev/ttyUSB2,hex --config TEMPLATE=merge"
    );
    println!(
        "  {bin_name} route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3 --no-log"
    );
    println!("  {bin_name} route --list-templates");
}

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}
