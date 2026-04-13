use std::process::ExitCode;

pub(crate) fn print_usage(bin_name: &str) {
    eprintln!("Usage: {bin_name} <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  control    Read DUALSHOCK 4 input and send serial output");
    eprintln!("  monitor    Monitor one or more serial ports");
    eprintln!("  controllers List connected DUALSHOCK 4 controllers");
    eprintln!("  ports      List available serial ports");
    eprintln!("  help       Show help for a command");
    eprintln!();
    eprintln!("Use `{bin_name} help control` or `{bin_name} help monitor` for details.");
}

pub(crate) fn print_help(bin_name: &str) {
    println!("Usage: {bin_name} <COMMAND>");
    println!();
    println!("Commands:");
    println!("  control    Read DUALSHOCK 4 input and send serial output");
    println!("  monitor    Monitor one or more serial ports");
    println!("  controllers List connected DUALSHOCK 4 controllers");
    println!("  ports      List available serial ports");
    println!("  help       Show help for a command");
    println!();
    println!("Examples:");
    println!("  {bin_name} control --port /dev/ttyUSB0 --baud 115200 --format arm9");
    println!("  {bin_name} control --monitor /dev/ttyUSB1");
    println!("  {bin_name} monitor --port /dev/ttyUSB0 --port /dev/ttyUSB1");
    println!("  {bin_name} control --config acs.config.json");
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
    println!("  -b, --baud <BAUD_RATE>     Serial baud rate (default: 115200)");
    println!("  -c, --controller <ID>      Controller index or HID path");
    println!("  -f, --format <FORMAT>      Output format (currently: arm9)");
    println!("      --raw                  Show incoming serial data as raw chunks");
    println!("      --display <TARGET=MODE> Display mode for a port");
    println!("                              TARGET: PORT, input:PORT, output:PORT,");
    println!("                                      default, input:default, output:default");
    println!("                             MODE: hex/ascii/utf8/hex+ascii/hex+utf8");
    println!("      --monitor <PORT>       Additional serial port to monitor");
    println!(
        "      --config <PATH>        Read options from a JSON file (default: ./acs.config.json if present)"
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
    println!("  {bin_name} control --config acs.config.json");
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
        "      --config <PATH>        Read options from a JSON file (default: ./acs.config.json if present)"
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
    println!("  {bin_name} monitor --config acs.config.json");
}

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}
