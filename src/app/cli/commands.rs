use crate::input::ds4_hid;
use crate::serial;
use serialport::{SerialPortInfo, SerialPortType};
use std::process::ExitCode;

pub(crate) fn list_controllers() -> ExitCode {
    match ds4_hid::list_devices() {
        Ok(devices) if devices.is_empty() => {
            println!("No DUALSHOCK 4 devices found");
            ExitCode::SUCCESS
        }
        Ok(devices) => {
            for (index, device) in devices.iter().enumerate() {
                println!("{}", format_device_line(index, device));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to list DUALSHOCK 4 devices: {error}");
            ExitCode::from(1)
        }
    }
}

fn format_device_line(index: usize, device: &ds4_hid::Ds4DeviceInfo) -> String {
    format!(
        "[{index}] transport={} vid=0x{:04x} pid=0x{:04x} interface={} product={} path={}",
        device.transport,
        device.vendor_id,
        device.product_id,
        device.interface_number,
        device.product_name.as_deref().unwrap_or("unknown"),
        device.path
    )
}

pub(crate) fn list_ports() -> ExitCode {
    match serial::available_ports() {
        Ok(ports) if ports.is_empty() => {
            println!("No serial ports found");
            ExitCode::SUCCESS
        }
        Ok(ports) => {
            for (index, port) in ports.iter().enumerate() {
                println!("{}", format_output_port(index, port));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to list serial output ports: {error}");
            ExitCode::from(1)
        }
    }
}

fn format_output_port(index: usize, port: &SerialPortInfo) -> String {
    let port_type = match &port.port_type {
        SerialPortType::UsbPort(info) => {
            format!(
                "type=usb vid=0x{:04x} pid=0x{:04x} manufacturer={} product={} serial={}",
                info.vid,
                info.pid,
                info.manufacturer.as_deref().unwrap_or("unknown"),
                info.product.as_deref().unwrap_or("unknown"),
                info.serial_number.as_deref().unwrap_or("unknown")
            )
        }
        SerialPortType::BluetoothPort => String::from("type=bluetooth"),
        SerialPortType::PciPort => String::from("type=pci"),
        SerialPortType::Unknown => String::from("type=unknown"),
    };

    format!("[{index}] {} {}", port.port_name, port_type)
}
