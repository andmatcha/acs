use std::process::ExitCode;

pub(crate) fn print_usage(bin_name: &str) {
    eprintln!("Usage: {bin_name} <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  control    Read DUALSHOCK 4 input and send serial output");
    eprintln!("  monitor    Monitor one or more serial ports");
    eprintln!("  route      Route bytes between serial inputs and outputs");
    eprintln!("  send       Repeatedly send dummy payloads to a serial port");
    eprintln!("  controllers List connected DUALSHOCK 4 controllers");
    eprintln!("  ports      List available serial ports");
    eprintln!("  help       Show help for a command");
    eprintln!();
    eprintln!(
        "Use `{bin_name} help control`, `{bin_name} help monitor`, `{bin_name} help route`, or `{bin_name} help send` for details."
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
    println!("  controllers List connected DUALSHOCK 4 controllers");
    println!("  ports      List available serial ports");
    println!("  help       Show help for a command");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --baud 115200 --format arm9");
    println!("  {bin_name} control --monitor /dev/ttyUSB1");
    println!("  {bin_name} monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1");
    println!("  {bin_name} route merge -i in_a=/dev/ttyUSB0 -o out_main=/dev/ttyUSB1");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketACv6");
    println!("  {bin_name} control --config config");
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
    println!("  -p, --port <PORT>          Serial output port");
    println!("                              Omit to auto-select a single USB serial or ST-LINK");
    println!("  -b, --baud <BAUD_RATE>     Serial baud rate (default: 115200)");
    println!("  -c, --controller <ID>      Controller index or HID path");
    println!("  -f, --format <FORMAT>      Output format (currently: arm9, packetacv6)");
    println!("      --raw                  Show incoming serial data as raw chunks");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                             MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("      --monitor <PORT>       Additional serial port to monitor");
    println!(
        "      --config <PATH>        Read options from a JSON file or directory (default: ./config/, fallback: ./acs.config.json)"
    );
    println!("      --log-dir <DIR>        Log directory (default: ./logs)");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --baud 115200 --format arm9");
    println!("  {bin_name} control --raw --monitor /dev/ttyUSB1");
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
    println!("Stops on Ctrl-C. The send interval matches `control` (20 ms).");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT>          Serial output port");
    println!("  -b, --baud <BAUD_RATE>     Serial baud rate (default: 115200)");
    println!("  -f, --format <FORMAT>      Dummy payload format (currently: arm9, packetacv6)");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format arm9");
    println!("  {bin_name} send --port /dev/ttyUSB0 --format PacketACv6");
}

pub(crate) fn print_monitor_help(bin_name: &str) {
    println!("Usage: {bin_name} monitor [OPTIONS]");
    println!();
    println!("Monitors one or more serial ports and displays the most recent 10 entries per port.");
    println!();
    println!("Options:");
    println!("  -p, --port <PORT>          Serial port to monitor (repeatable)");
    println!("  -b, --baud <BAUD_RATE>     Serial baud rate (default: 115200)");
    println!("      --raw                  Show incoming serial data as raw chunks");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                             MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!(
        "      --config <PATH>        Read options from a JSON file or directory (default: ./config/, fallback: ./acs.config.json)"
    );
    println!("      --log-dir <DIR>        Log directory (default: ./logs)");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  {bin_name} monitor --port /dev/ttyUSB0");
    println!("  {bin_name} monitor --raw --port /dev/ttyUSB0");
    println!(
        "  {bin_name} monitor --display input:/dev/ttyUSB0=utf8 --display input:default=hex+utf8"
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
    println!("  -i, --input-port <ID=PORT>  Route input port (repeatable)");
    println!("  -o, --output-port <ID=PORT> Route output port (repeatable)");
    println!("  -b, --baud <BAUD_RATE>      Default baud rate for CLI-specified ports");
    println!("      --raw                   Show incoming serial data as raw chunks");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                              MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!(
        "      --config <PATH>         Read options from a JSON file or directory (default: ./config/, fallback: ./acs.config.json)"
    );
    println!("      --log-dir <DIR>         Log directory (default: ./logs)");
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
        "  {bin_name} route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3"
    );
    println!("  {bin_name} route --list-templates");
    println!("  {bin_name} route --config config");
}

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}
