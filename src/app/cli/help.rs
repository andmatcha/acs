use std::process::ExitCode;

pub(crate) fn print_usage(bin_name: &str) {
    eprintln!("Usage: {bin_name} <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  control    Read DUALSHOCK 4 input and send serial output");
    eprintln!("  monitor    Monitor one or more serial ports");
    eprintln!("  route      Route bytes between serial inputs and outputs");
    eprintln!("  send       Repeatedly send dummy payloads to a serial port");
    eprintln!("  xbee-mock  Run one side of the xbee-test traffic model on a single port");
    eprintln!("  xbee-rtt   Check connectivity and round-trip time across one XBee pair");
    eprintln!("  xbee-test  Cross-test AU/RU and AD/RD across base/remote ports");
    eprintln!("  controllers List connected DUALSHOCK 4 controllers");
    eprintln!("  ports      List available serial ports");
    eprintln!("  version    Show build version and source metadata");
    eprintln!("  help       Show help for a command");
    eprintln!();
    eprintln!("Use `{bin_name} --version` or `{bin_name} version` to inspect the installed build.");
    eprintln!();
    eprintln!(
        "Use `{bin_name} help control`, `{bin_name} help monitor`, `{bin_name} help route`, `{bin_name} help send`, `{bin_name} help xbee-mock`, `{bin_name} help xbee-rtt`, or `{bin_name} help xbee-test` for details."
    );
}

pub(crate) fn print_help(bin_name: &str) {
    println!("Usage: {bin_name} <COMMAND>");
    println!();
    println!("Commands:");
    println!("  control    Read DUALSHOCK 4 input and send serial output");
    println!("  monitor    Monitor one or more serial ports");
    println!("  route      Route bytes between serial inputs and outputs");
    println!("  send       Repeatedly send dummy payloads to a serial port");
    println!("  xbee-mock  Run one side of the xbee-test traffic model on a single port");
    println!("  xbee-rtt   Check connectivity and round-trip time across one XBee pair");
    println!("  xbee-test  Cross-test AU/RU and AD/RD across base/remote ports");
    println!("  controllers List connected DUALSHOCK 4 controllers");
    println!("  ports      List available serial ports");
    println!("  version    Show build version and source metadata");
    println!("  help       Show help for a command");
    println!();
    println!("Examples:");
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
    println!(
        "When exactly one controller or one serial port is available, it is selected automatically."
    );
    println!("If baud is omitted on any port option, 115200 is used.");
    println!("Most non-boolean command settings can also be grouped with `--config KEY=VALUE,...`.");
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
            println!("Usage: {bin_name} controllers");
            println!();
            println!(
                "Lists connected DUALSHOCK 4 controllers and their transport, VID/PID, interface, product name, and path."
            );
            ExitCode::SUCCESS
        }
        Some("ports") => {
            println!("Usage: {bin_name} ports");
            println!();
            println!("Lists available serial ports and USB metadata when available.");
            ExitCode::SUCCESS
        }
        Some("version") => {
            println!("Usage: {bin_name} --version");
            println!("       {bin_name} version");
            println!();
            println!(
                "Shows the package version plus build source metadata such as commit, branch, source kind, and dirty/clean state."
            );
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown help topic: {other}");
            print_help(bin_name);
            ExitCode::from(2)
        }
    }
}

pub(crate) fn print_control_help(bin_name: &str) {
    println!("Usage: {bin_name} control [OPTIONS]");
    println!();
    println!("Reads a DUALSHOCK 4 controller and writes formatted bytes to a serial port.");
    println!(
        "The output port is also monitored as input, and extra ports can be added with `--monitor`."
    );
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> Serial output port");
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!("                              Omit to auto-select a single USB serial or ST-LINK");
    println!("  -m, --monitor <PORT[@BAUD][,DISPLAY]> Additional serial port to monitor");
    println!("      --config <K=V,...>     Grouped settings: `CONTROLLER`, `FORMAT`, `DISPLAY`, `LOG_DIR`");
    println!("                              `FORMAT`: output format (currently: packetacv6)");
    println!("                              `DISPLAY`: display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --config FORMAT=PacketACv6");
    println!(
        "  {bin_name} control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8 --config FORMAT=PacketACv6"
    );
    println!("  {bin_name} control --config DISPLAY=input:default=utf8+packet --monitor /dev/ttyUSB1");
    println!(
        "  {bin_name} control --config DISPLAY=input:/dev/ttyUSB0=utf8 --config DISPLAY=output:/dev/ttyUSB0=hex"
    );
    println!("  {bin_name} control --config CONTROLLER=0 --monitor /dev/ttyUSB1 --no-log");
}

pub(crate) fn print_send_help(bin_name: &str) {
    println!("Usage: {bin_name} send [OPTIONS]");
    println!();
    println!("Repeatedly sends dummy payloads in the selected format to a serial port.");
    println!("With --interactive, accepts terminal input and sends each line on Enter.");
    println!("Sent packets are displayed live like `monitor`. Stops on Ctrl-C.");
    println!("Default send rate is 50 Hz, which matches `control`'s 20 ms interval.");
    println!(
        "Known mixed-format input ports are decoded per format, and each format's RX Hz is shown in the header."
    );
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> Serial output port");
    println!(
        "  -o, --output-port <ID=PORT[@BAUD][,DISPLAY][,FORMAT][,RATE]> Additional/repeatable serial output"
    );
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!(
        "      --config <K=V,...>     Grouped settings: `FORMAT`, `RATE`, `DISPLAY`, `LOG_DIR`"
    );
    println!("                              `RATE` is ignored with --interactive");
    println!("                              `DISPLAY` uses TARGET=MODE, for example `output:default=hex`");
    println!(
        "  -i, --interactive          Read lines from terminal and send on Enter (raw UTF-8 + \\r\\n)"
    );
    println!(
        "  -m, --monitor <PORT[@BAUD][,DISPLAY][,FORMAT[+FORMAT...]]> Additional serial port to monitor"
    );
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketACv6");
    println!("  {bin_name} send --port /dev/ttyUSB0@921600 --config FORMAT=PacketACv6,RATE=100");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=PacketJFv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverUpGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0 --config FORMAT=RoverDownGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0,hex --monitor /dev/ttyUSB1,utf8,packetjfv1 --config DISPLAY=output:default=hex");
    println!("  {bin_name} send --port /dev/ttyUSB0 --monitor /dev/ttyUSB1,packetacv6+packetjfv1 --config FORMAT=PacketACv6");
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

pub(crate) fn print_xbee_test_help(bin_name: &str) {
    println!("Usage: {bin_name} xbee-test [OPTIONS]");
    println!();
    println!(
        "Sends AU(PacketACv6) + RU(RoverUpGeneral) from `base` to `remote`, and AD(PacketJFv1) + RD(RoverDownGeneral) from `remote` to `base`."
    );
    println!("Modes: `flood`, `ping-pong`, and `polling`.");
    println!(
        "Each port is monitored simultaneously, and the dashboard shows per-port TX/RX packet rates plus matched/error statistics."
    );
    println!(
        "Input and output are fixed to hex packet display for lightweight high-rate monitoring."
    );
    println!(
        "The header also shows the actual display FPS, while RX rate/error counters continue to track packets independently of terminal refresh speed."
    );
    println!();
    println!("Options:");
    println!("  -p, --port <ID=PORT[@BAUD]> Port binding. IDs: `base`, `remote`");
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!("      --config <K=V,...>     Grouped settings: `MODE`, `POLL_RATE`, `BASE_REAL_PERCENT`,");
    println!("                              `REMOTE_REAL_PERCENT`, `AU_RATE`, `RU_RATE`, `AD_RATE`, `RD_RATE`, `LOG_DIR`");
    println!("                              `MODE`: `flood`, `ping-pong`, or `polling` (default: flood)");
    println!("                              `AD_RATE` / `RD_RATE` are ignored in `ping-pong`");
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
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
    println!("Usage: {bin_name} xbee-rtt [OPTIONS]");
    println!();
    println!(
        "Runs a symmetric connectivity check and bidirectional RTT measurement across one XBee pair."
    );
    println!(
        "The normal mode is one local serial port per PC: run the same command on both PCs, and the peers negotiate who measures first."
    );
    println!(
        "Only when `--port` is given twice does one process drive two local XBee modules and treat them as a local pair."
    );
    println!(
        "A compact binary frame with negotiated session ID, length, and CRC16 is used so the decoder can resynchronize after noise."
    );
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD]>    Local serial port for one XBee module");
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!("                              Omit to auto-select one serial port");
    println!("                              Specify twice only for one-PC local-pair mode");
    println!("      --config <K=V,...>      Grouped settings: `PAYLOAD_SIZE`, `COUNT`, `INTERVAL_MS`,");
    println!("                              `PROBE_TIMEOUT_MS`, `CONNECT_TIMEOUT_MS`");
    println!(
        "      --show-wire            Continuously dump live TX/RX bytes in hex; TX is blue, RX is red"
    );
    println!(
        "                            In one-PC local-pair mode, chunks are prefixed as [0>] / [1<]"
    );
    println!(
        "      --show-protocol        Continuously dump decoded HELLO / PROBE / RESULT-style logs"
    );
    println!(
        "                            Uses the same blue/red color split and [0>] / [1<] prefixes"
    );
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} xbee-rtt --port /dev/ttyUSB0");
    println!("  # Run the same command on the other PC too");
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
    println!("Usage: {bin_name} xbee-mock <ROLE> [OPTIONS]");
    println!("       {bin_name} xbee-mock --role <ROLE> [OPTIONS]");
    println!();
    println!(
        "Runs either the `base` side or the `remote` side of the xbee traffic model with explicit uplink/downlink bindings."
    );
    println!(
        "With `PAIR=1`, one port is shared for both directions. With `PAIR=2`, `up` and `down` are bound separately."
    );
    println!("Display is fixed to hex packet mode for lightweight high-rate monitoring.");
    println!();
    println!("Roles:");
    println!("  base    Uplink is TX, downlink is RX");
    println!("  remote  Uplink is RX, downlink is TX");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD]>   Port binding for `PAIR=1`");
    println!("  -p, --port <up=PORT[@BAUD]> Uplink binding for `PAIR=2`");
    println!("  -p, --port <down=PORT[@BAUD]> Downlink binding for `PAIR=2`");
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!(
        "      --config <K=V,...>     Grouped settings: `ROLE`, `PAIR`, `TX_FORMAT`, `RX_FORMAT`, `TRAFFIC_PATTERN`, `LOG_DIR`"
    );
    println!("                              `PAIR`: `1` or `2` (default: 1)");
    println!("                              `TX_FORMAT`: `<FORMAT[@RATE]>[+<FORMAT[@RATE]>...]`");
    println!("                              `RX_FORMAT`: `<FORMAT>[+<FORMAT>...]`");
    println!("                              `TRAFFIC_PATTERN`: `flood`, `ping-pong`, or `polling`");
    println!("                              Poll formats are `PollGreeting` / `PollResponse`");
    println!("                              Legacy alias: `--option`");
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
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
    println!("Usage: {bin_name} monitor [OPTIONS]");
    println!();
    println!("Monitors one or more serial ports and displays the most recent 10 entries per port.");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> Serial port to monitor (repeatable)");
    println!("                              PORT accepts a device path or `acs ports` index");
    println!("                              BAUD defaults to 115200 when omitted");
    println!("      --config <K=V,...>     Grouped settings: `DISPLAY`, `LOG_DIR`");
    println!("                              `DISPLAY`: display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} monitor --port /dev/ttyUSB0");
    println!("  {bin_name} monitor --port /dev/ttyUSB0,utf8+packet --port /dev/ttyUSB1,hex");
    println!(
        "  {bin_name} monitor --config DISPLAY=input:/dev/ttyUSB0=utf8+packet --config DISPLAY=input:default=hex+utf8+line"
    );
    println!("  {bin_name} monitor --port /dev/ttyUSB0@921600 --port /dev/ttyUSB1@115200 --no-log");
}

pub(crate) fn print_route_help(bin_name: &str) {
    println!("Usage: {bin_name} route [TEMPLATE] [OPTIONS]");
    println!();
    println!("Routes bytes from one or more serial inputs to one or more serial outputs.");
    println!("Routing behavior can be selected from built-in route templates.");
    println!();
    println!("Options:");
    println!("      --list-templates        Show built-in route templates");
    println!("  -i, --input-port <ID=PORT[@BAUD][,DISPLAY]>  Route input port (repeatable)");
    println!("  -o, --output-port <ID=PORT[@BAUD][,DISPLAY]> Route output port (repeatable)");
    println!("                               PORT accepts a device path or `acs ports` index");
    println!("                               BAUD defaults to 115200 when omitted");
    println!("      --config <K=V,...>      Grouped settings: `TEMPLATE`, `DISPLAY`, `LOG_DIR`");
    println!("                              `TEMPLATE` is the same as the positional TEMPLATE");
    println!("                              `DISPLAY`: display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --no-log                Disable log file creation for maximum throughput");
    println!("                              Value-taking legacy flags remain available for compatibility");
    println!("  -h, --help                  Show this help");
    println!();
    println!("Built-in templates:");
    println!("  merge            Forward bytes in arrival order to all outputs");
    println!("  one-to-one       Pair input/output arrays by order and pass bytes through");
    println!();
    println!("Examples:");
    println!("  {bin_name} route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1");
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0@921600 -i in_b=/dev/ttyUSB1@115200 -o out_main=/dev/ttyUSB2@921600"
    );
    println!("  {bin_name} route -i in_a=/dev/ttyUSB0,utf8 -o out_main=/dev/ttyUSB2,hex --config TEMPLATE=merge");
    println!(
        "  {bin_name} route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3 --no-log"
    );
    println!("  {bin_name} route --list-templates");
}

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}
