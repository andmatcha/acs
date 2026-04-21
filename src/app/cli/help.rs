use super::paths;
use std::process::ExitCode;

pub(crate) fn print_usage(bin_name: &str) {
    eprintln!("Usage: {bin_name} <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  control    Read DUALSHOCK 4 input and send serial output");
    eprintln!("  monitor    Monitor one or more serial ports");
    eprintln!("  route      Route bytes between serial inputs and outputs");
    eprintln!("  send       Repeatedly send dummy payloads to a serial port");
    eprintln!("  xbee-test  Cross-test PacketACv6 and PacketJFv1 across base/rover ports");
    eprintln!("  controllers List connected DUALSHOCK 4 controllers");
    eprintln!("  ports      List available serial ports");
    eprintln!("  version    Show build version and source metadata");
    eprintln!("  help       Show help for a command");
    eprintln!();
    eprintln!("Use `{bin_name} --version` or `{bin_name} version` to inspect the installed build.");
    eprintln!();
    eprintln!(
        "Use `{bin_name} help control`, `{bin_name} help monitor`, `{bin_name} help route`, `{bin_name} help send`, or `{bin_name} help xbee-test` for details."
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
    println!("  xbee-test  Cross-test PacketACv6 and PacketJFv1 across base/rover ports");
    println!("  controllers List connected DUALSHOCK 4 controllers");
    println!("  ports      List available serial ports");
    println!("  version    Show build version and source metadata");
    println!("  help       Show help for a command");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6");
    println!(
        "  {bin_name} control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8"
    );
    println!(
        "  {bin_name} monitor --port /dev/ttyUSB0@921600,utf8+packet --port /dev/ttyUSB1@115200,hex"
    );
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0@921600,utf8 -o out_main=/dev/ttyUSB1@115200,hex"
    );
    println!("  {bin_name} send --port /dev/ttyUSB0@921600,hex --format PacketACv6");
    println!(
        "  {bin_name} send -o main=/dev/ttyUSB0@921600,hex,packetacv6 -o sub=/dev/ttyUSB1@115200,utf8,packetjfv1"
    );
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketJFv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format RoverUpGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format RoverDownGeneral");
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --ac-rate 100 --jf-rate 100"
    );
    println!("  {bin_name} control --config config");
    println!("  {bin_name} --version");
    println!();
    println!(
        "When exactly one controller or one serial port is available, it is selected automatically."
    );
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
    println!("                              Omit to auto-select a single USB serial or ST-LINK");
    println!("  -b, --baud <BAUD_RATE>     Default baud rate (default: 115200)");
    println!("  -c, --controller <ID>      Controller index or HID path");
    println!("  -f, --format <FORMAT>      Output format (currently: packetacv6)");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --monitor <PORT[@BAUD][,DISPLAY]> Additional serial port to monitor");
    println!("      --config <PATH>        Read options from a JSON file or directory");
    println!(
        "                              Defaults: {}",
        paths::default_config_help()
    );
    println!(
        "      --log-dir <DIR>        Log directory (default: {})",
        paths::default_log_help()
    );
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --baud 115200 --format PacketACv6");
    println!(
        "  {bin_name} control --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8"
    );
    println!("  {bin_name} control --display input:default=utf8+packet --monitor /dev/ttyUSB1");
    println!(
        "  {bin_name} control --display input:/dev/ttyUSB0=utf8 --display output:/dev/ttyUSB0=hex"
    );
    println!("  {bin_name} control --controller 0 --monitor /dev/ttyUSB1");
    println!("  {bin_name} control --config config");
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
    println!("  -b, --baud <BAUD_RATE>     Default baud rate (default: 115200)");
    println!(
        "  -r, --rate <HZ>            Default dummy packet send rate (default: 50, overridden by output-port rate, ignored with --interactive)"
    );
    println!(
        "  -f, --format <FORMAT>      Dummy payload format (currently: packetacv6, packetjfv1, roverupgeneral, roverdowngeneral)"
    );
    println!(
        "  -i, --interactive          Read lines from terminal and send on Enter (raw UTF-8 + \\r\\n)"
    );
    println!(
        "  -m, --monitor <PORT[@BAUD][,DISPLAY][,FORMAT[+FORMAT...]]> Additional serial port to monitor"
    );
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!(
        "                              TARGET: PORT, input:PORT, output:PORT, default, input:default, output:default"
    );
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --config <PATH>        Read options from a JSON file or directory");
    println!(
        "                              Defaults: {}",
        paths::default_config_help()
    );
    println!(
        "      --log-dir <DIR>        Log directory (default: {})",
        paths::default_log_help()
    );
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketACv6");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketACv6 --rate 100");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketJFv1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format RoverUpGeneral");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format RoverDownGeneral");
    println!(
        "  {bin_name} send --port /dev/ttyUSB0@921600,hex --monitor /dev/ttyUSB1@115200,utf8,packetjfv1"
    );
    println!(
        "  {bin_name} send --port /dev/ttyUSB0 --monitor /dev/ttyUSB1@115200,packetacv6+packetjfv1"
    );
    println!("  {bin_name} send --port /dev/ttyUSB0 --monitor /dev/ttyUSB1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketACv6 --no-log");
    println!(
        "  {bin_name} send -o main=/dev/ttyUSB0@921600,hex,packetacv6 -o sub=/dev/ttyUSB1@115200,utf8+packet,packetjfv1"
    );
    println!(
        "  {bin_name} send -o ac=/dev/ttyUSB0@921600,hex,packetacv6,100 -o up=/dev/ttyUSB0@921600,utf8,roverupgeneral,10"
    );
    println!("  {bin_name} send --interactive --port /dev/ttyUSB0@115200");
    println!("  {bin_name} send -i --port /dev/ttyUSB0 --monitor /dev/ttyUSB1");
    println!("  {bin_name} send --display output:default=hex --display input:default=utf8+packet");
    println!("  {bin_name} send --config config");
}

pub(crate) fn print_xbee_test_help(bin_name: &str) {
    println!("Usage: {bin_name} xbee-test [OPTIONS]");
    println!();
    println!(
        "Sends PacketACv6 from `base` to `rover` and PacketJFv1 from `rover` to `base` at independent rates."
    );
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
    println!("  -p, --port <ID=PORT[@BAUD]> Port binding. IDs: `base`, `rover`");
    println!("      --mode <MODE>          Transfer mode: `flood` or `ping-pong` (default: flood)");
    println!(
        "      --ac-rate <HZ>         PacketACv6 send rate from `base` to `rover` (default: 100)"
    );
    println!(
        "      --jf-rate <HZ>         PacketJFv1 send rate from `rover` to `base` (default: 100)"
    );
    println!(
        "                              Ignored in `ping-pong`; JF is sent once per valid AC receive"
    );
    println!("      --config <PATH>        Read options from a JSON file or directory");
    println!(
        "                              Defaults: {}",
        paths::default_config_help()
    );
    println!(
        "      --log-dir <DIR>        Log directory (default: {})",
        paths::default_log_help()
    );
    println!("      --no-log               Disable log file creation for maximum throughput");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200"
    );
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --ac-rate 100 --jf-rate 50"
    );
    println!(
        "  {bin_name} xbee-test --mode ping-pong --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --ac-rate 100"
    );
    println!(
        "  {bin_name} xbee-test --port base=/dev/ttyUSB0@921600 --port rover=/dev/ttyUSB1@115200 --no-log"
    );
    println!("  {bin_name} xbee-test --config config");
}

pub(crate) fn print_monitor_help(bin_name: &str) {
    println!("Usage: {bin_name} monitor [OPTIONS]");
    println!();
    println!("Monitors one or more serial ports and displays the most recent 10 entries per port.");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT[@BAUD][,DISPLAY]> Serial port to monitor (repeatable)");
    println!("  -b, --baud <BAUD_RATE>     Default baud rate (default: 115200)");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --config <PATH>        Read options from a JSON file or directory");
    println!(
        "                              Defaults: {}",
        paths::default_config_help()
    );
    println!(
        "      --log-dir <DIR>        Log directory (default: {})",
        paths::default_log_help()
    );
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} monitor --port /dev/ttyUSB0");
    println!(
        "  {bin_name} monitor --port /dev/ttyUSB0@921600,utf8+packet --port /dev/ttyUSB1@115200,hex"
    );
    println!(
        "  {bin_name} monitor --display input:/dev/ttyUSB0=utf8+packet --display input:default=hex+utf8+line"
    );
    println!("  {bin_name} monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1");
    println!("  {bin_name} monitor --config config");
}

pub(crate) fn print_route_help(bin_name: &str) {
    println!("Usage: {bin_name} route [TEMPLATE] [OPTIONS]");
    println!();
    println!("Routes bytes from one or more serial inputs to one or more serial outputs.");
    println!(
        "Routing behavior can be defined directly in config, or selected from route templates."
    );
    println!();
    println!("Options:");
    println!("      --template <NAME>       Route template name (same as positional TEMPLATE)");
    println!("      --list-templates        Show built-in and config-defined route templates");
    println!("  -i, --input-port <ID=PORT[@BAUD][,DISPLAY]>  Route input port (repeatable)");
    println!("  -o, --output-port <ID=PORT[@BAUD][,DISPLAY]> Route output port (repeatable)");
    println!("  -b, --baud <BAUD_RATE>      Default baud rate for ports without inline baud");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("                                    + optional +line/+packet for monitor input");
    println!("      --config <PATH>         Read options from a JSON file or directory");
    println!(
        "                               Defaults: {}",
        paths::default_config_help()
    );
    println!(
        "      --log-dir <DIR>         Log directory (default: {})",
        paths::default_log_help()
    );
    println!("  -h, --help                  Show this help");
    println!();
    println!("Built-in templates:");
    println!("  merge            Forward bytes in arrival order to all outputs");
    println!("  one-to-one       Pair input/output arrays by order and pass bytes through");
    println!();
    println!("Examples:");
    println!("  {bin_name} route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1");
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_main=/dev/ttyUSB2"
    );
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0@921600,utf8 -o out_main=/dev/ttyUSB2@115200,hex"
    );
    println!(
        "  {bin_name} route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3"
    );
    println!("  {bin_name} route --list-templates");
    println!("  {bin_name} route --config config");
}

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}
