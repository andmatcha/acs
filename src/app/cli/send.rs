use super::common::{default_baud_rate, next_value, parse_u32_arg};
use super::help::{is_help_flag, print_send_help};
use super::signal;
use crate::common::format_bytes_hex;
use crate::output::OutputFormat;
use crate::serial::{self, SerialConfig, SerialWriter};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const SEND_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
struct SendCliOptions {
    port: Option<String>,
    baud: Option<u32>,
    format: Option<String>,
}

struct SendSettings {
    port: String,
    baud: u32,
    format: OutputFormat,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_send_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_send_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_send_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok((settings, payload, sent_count)) => {
            println!(
                "sent {} packets ({} bytes each) to {} @ {} baud, format={}",
                sent_count,
                payload.len(),
                settings.port,
                settings.baud,
                settings.format.as_str()
            );
            println!("{}", format_bytes_hex(&payload));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: SendCliOptions) -> Result<(SendSettings, Vec<u8>, u64), String> {
    let settings = build_settings(cli_options)?;
    let payload = settings.format.encode_dummy_payload()?;

    let mut writer = SerialWriter::open(&SerialConfig {
        port: settings.port.clone(),
        baud_rate: settings.baud,
    })
    .map_err(|error| error.to_string())?;
    let mut sent_count = 0u64;

    signal::install_handler();
    while !signal::is_stop_requested() {
        writer
            .write_bytes(&payload)
            .map_err(|error| error.to_string())?;
        sent_count = sent_count.saturating_add(1);
        thread::sleep(SEND_INTERVAL);
    }

    Ok((settings, payload, sent_count))
}

fn build_settings(cli_options: SendCliOptions) -> Result<SendSettings, String> {
    let port =
        serial::resolve_port(cli_options.port.as_deref()).map_err(|error| error.to_string())?;
    let baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let format_name = cli_options.format.unwrap_or_else(|| String::from("arm9"));
    let format = OutputFormat::parse(&format_name)?;

    Ok(SendSettings { port, baud, format })
}

fn parse_send_args(args: Vec<String>) -> Result<SendCliOptions, String> {
    let mut options = SendCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.port = Some(next_value(&mut iter, "--port")?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            other => return Err(format!("unknown option for send: {other}")),
        }
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::parse_send_args;

    #[test]
    fn parse_send_args_accepts_port_baud_and_format() {
        let options = parse_send_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0"),
            String::from("--baud"),
            String::from("921600"),
            String::from("--format"),
            String::from("PacketACv6"),
        ])
        .expect("should parse");

        assert_eq!(options.port.as_deref(), Some("/dev/ttyUSB0"));
        assert_eq!(options.baud, Some(921_600));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
    }
}
