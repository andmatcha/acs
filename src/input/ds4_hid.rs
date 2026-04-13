use hidapi::{BusType, HidApi, HidDevice, HidError};
use std::fmt;

const SONY_VENDOR_ID: u16 = 0x054c;
const DUALSHOCK_4_PRODUCT_IDS: [u16; 2] = [0x05c4, 0x09cc];
// USB reports fit in 64 bytes, while Bluetooth input reports are larger.
const MAX_REPORT_SIZE: usize = 128;

#[derive(Debug, Clone)]
pub struct Ds4DeviceInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: i32,
    pub product_name: Option<String>,
    pub transport: &'static str,
}

#[derive(Debug)]
pub enum Ds4Error {
    Hid(HidError),
    DeviceNotFound,
    MultipleDevicesFound(usize),
    DeviceSelectionNotFound(String),
}

impl fmt::Display for Ds4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hid(error) => write!(f, "{error}"),
            Self::DeviceNotFound => write!(f, "no compatible DUALSHOCK 4 was found"),
            Self::MultipleDevicesFound(count) => write!(
                f,
                "{count} DUALSHOCK 4 devices found; specify one with --controller"
            ),
            Self::DeviceSelectionNotFound(selector) => write!(
                f,
                "controller `{selector}` was not found; use `acs controllers` to inspect candidates"
            ),
        }
    }
}

impl From<HidError> for Ds4Error {
    fn from(value: HidError) -> Self {
        Self::Hid(value)
    }
}

pub fn list_devices() -> Result<Vec<Ds4DeviceInfo>, Ds4Error> {
    let api = HidApi::new()?;
    Ok(device_infos(&api))
}

pub struct Ds4Controller {
    _api: HidApi,
    device: HidDevice,
    info: Ds4DeviceInfo,
}

impl Ds4Controller {
    pub fn open(selector: Option<&str>) -> Result<Self, Ds4Error> {
        let api = HidApi::new()?;
        let device_info = select_device_info(&api, selector)?;
        let info = map_device_info(device_info);
        let device = device_info.open_device(&api)?;
        Ok(Self {
            _api: api,
            device,
            info,
        })
    }

    pub fn info(&self) -> &Ds4DeviceInfo {
        &self.info
    }

    pub fn read_next_report(
        &mut self,
        timeout_millis: i32,
    ) -> Result<Option<Vec<u8>>, Ds4Error> {
        read_next_report_with_timeout(&self.device, timeout_millis)
    }
}

fn device_infos(api: &HidApi) -> Vec<Ds4DeviceInfo> {
    api.device_list()
        .filter(|device| is_dualshock_4(device.vendor_id(), device.product_id()))
        .map(map_device_info)
        .collect()
}

fn select_device_info<'a>(
    api: &'a HidApi,
    selector: Option<&str>,
) -> Result<&'a hidapi::DeviceInfo, Ds4Error> {
    let devices = api
        .device_list()
        .filter(|device| is_dualshock_4(device.vendor_id(), device.product_id()))
        .collect::<Vec<_>>();

    match selector {
        Some(selector) => {
            if let Ok(index) = selector.parse::<usize>() {
                return devices
                    .get(index)
                    .copied()
                    .ok_or_else(|| Ds4Error::DeviceSelectionNotFound(selector.to_owned()));
            }

            devices
                .into_iter()
                .find(|device| device.path().to_string_lossy() == selector)
                .ok_or_else(|| Ds4Error::DeviceSelectionNotFound(selector.to_owned()))
        }
        None => match devices.as_slice() {
            [] => Err(Ds4Error::DeviceNotFound),
            [device] => Ok(*device),
            many => Err(Ds4Error::MultipleDevicesFound(many.len())),
        },
    }
}

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

fn map_device_info(device: &hidapi::DeviceInfo) -> Ds4DeviceInfo {
    Ds4DeviceInfo {
        path: device.path().to_string_lossy().into_owned(),
        vendor_id: device.vendor_id(),
        product_id: device.product_id(),
        interface_number: device.interface_number(),
        product_name: device.product_string().map(ToOwned::to_owned),
        transport: bus_type_label(device.bus_type()),
    }
}

fn bus_type_label(bus_type: BusType) -> &'static str {
    match bus_type {
        BusType::Usb => "usb",
        BusType::Bluetooth => "bluetooth",
        BusType::I2c => "i2c",
        BusType::Spi => "spi",
        BusType::Unknown => "unknown",
    }
}

fn is_dualshock_4(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == SONY_VENDOR_ID && DUALSHOCK_4_PRODUCT_IDS.contains(&product_id)
}
