mod definition;
mod encoder;
mod reduced;
mod sound;

use crate::input::compact::CompactReport;
use crate::output::formats::crc::crc16_ccitt_false;
use crate::output::formats::{DummyPayloadGenerator, OutputDriver};
use definition::{PACKET_ACV6_DEFINITION, PACKET_ACV6_PACKET_LEN, PACKET_ACV6_PAYLOAD_LEN};
use encoder::PacketAcV6PacketEncoder;
use reduced::ReducedAcPacketKind;
use sound::ModeSoundPlayer;

const PACKET_ACV6_CONTROL_BYTE_OFFSET: usize = 30;
const PACKET_MV1_PACKET_LEN: usize = 19;
const PACKET_MV1_PAYLOAD_LEN: usize = 17;
const PACKET_MV1_CONTROL_BYTE_OFFSET: usize = 16;
const CONTROL_BYTE_READ_USB: u8 = 1 << 5;

pub(crate) fn create_driver() -> Box<dyn OutputDriver> {
    Box::new(PacketAcV6OutputDriver::new())
}

pub(crate) fn create_packet_mv1_driver() -> Box<dyn OutputDriver> {
    Box::new(ReducedAcOutputDriver::new(ReducedAcPacketKind::PacketMv1))
}

pub(crate) const fn packet_len() -> usize {
    PACKET_ACV6_PACKET_LEN
}

pub(crate) fn set_usb_read_flag(packet: &mut [u8]) -> Result<(), String> {
    let (control_byte_offset, payload_len) = if packet.len() == PACKET_ACV6_PACKET_LEN {
        if !packet.starts_with(&PACKET_ACV6_DEFINITION.header) {
            return Err(String::from("packetacv6 usb_read flag expects AC header"));
        }
        (PACKET_ACV6_CONTROL_BYTE_OFFSET, PACKET_ACV6_PAYLOAD_LEN)
    } else if packet.len() == PACKET_MV1_PACKET_LEN {
        if packet[0] != b'M' {
            return Err(String::from("packetmv1 usb_read flag expects M header"));
        }
        (PACKET_MV1_CONTROL_BYTE_OFFSET, PACKET_MV1_PAYLOAD_LEN)
    } else {
        return Err(format!(
            "packetacv6 usb_read flag expects {PACKET_ACV6_PACKET_LEN} byte AC or {PACKET_MV1_PACKET_LEN} byte M packet, got {}",
            packet.len()
        ));
    };

    packet[control_byte_offset] |= CONTROL_BYTE_READ_USB;
    let crc = crc16_ccitt_false(&packet[..payload_len]);
    packet[payload_len..].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketAcV6DummyGenerator::default()))
}

pub(crate) fn create_usb_read_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketAcV6UsbReadDummyGenerator::default()))
}

pub(crate) const fn packet_mv1_len() -> usize {
    reduced::packet_len(ReducedAcPacketKind::PacketMv1)
}

pub(crate) const fn packet_iv1_len() -> usize {
    reduced::packet_len(ReducedAcPacketKind::PacketIv1)
}

pub(crate) const fn packet_bv1_len() -> usize {
    reduced::packet_len(ReducedAcPacketKind::PacketBv1)
}

pub(crate) fn create_packet_mv1_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String>
{
    reduced::create_dummy_generator(ReducedAcPacketKind::PacketMv1)
}

pub(crate) fn create_packet_iv1_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String>
{
    reduced::create_dummy_generator(ReducedAcPacketKind::PacketIv1)
}

pub(crate) fn create_packet_bv1_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String>
{
    reduced::create_dummy_generator(ReducedAcPacketKind::PacketBv1)
}

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

impl OutputDriver for PacketAcV6OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String> {
        let update = self.encoder.encode_compact_report_update(compact_report);
        if update.profile_changed {
            self.sound_player.play(update.profile.as_str());
        }
        Ok(update.packet.to_vec())
    }
}

struct ReducedAcOutputDriver {
    source_driver: PacketAcV6OutputDriver,
    kind: ReducedAcPacketKind,
}

impl ReducedAcOutputDriver {
    fn new(kind: ReducedAcPacketKind) -> Self {
        Self {
            source_driver: PacketAcV6OutputDriver::new(),
            kind,
        }
    }
}

impl OutputDriver for ReducedAcOutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String> {
        let ac_packet = self.source_driver.encode(compact_report)?;
        reduced::reduce_packetacv6_for_kind(&ac_packet, self.kind)
    }
}

struct PacketAcV6DummyGenerator {
    driver: PacketAcV6OutputDriver,
    step: u8,
}

impl Default for PacketAcV6DummyGenerator {
    fn default() -> Self {
        Self {
            driver: PacketAcV6OutputDriver::new(),
            step: 0,
        }
    }
}

impl DummyPayloadGenerator for PacketAcV6DummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let payload = self.driver.encode(&build_dummy_compact_report(self.step))?;
        self.step = self.step.wrapping_add(1);
        Ok(payload)
    }
}

#[derive(Default)]
struct PacketAcV6UsbReadDummyGenerator {
    encoder: PacketAcV6PacketEncoder,
}

impl DummyPayloadGenerator for PacketAcV6UsbReadDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let mut payload = self
            .encoder
            .encode_compact_report_update(&[0u8; 8])
            .packet
            .to_vec();
        set_usb_read_flag(&mut payload)?;
        Ok(payload)
    }
}

fn build_dummy_compact_report(step: u8) -> CompactReport {
    let mut report = [0u8; 8];
    report[2] = step.wrapping_mul(17);
    report[3] = 128;
    report[4] = step.wrapping_mul(29);
    report[5] = 128;

    if step == 0 {
        report[1] |= 1 << 3; // options: enable manual output once
    }
    if step % 8 == 4 {
        report[1] |= 1 << 2; // share: cycle profile on a predictable cadence
    }

    match step % 8 {
        0 => report[7] = 255,
        1 => report[6] = 255,
        2 => report[5] = 0,
        3 => report[5] = 255,
        4 => report[3] = 0,
        5 => report[3] = 255,
        6 => {
            report[0] |= 1 << 7;
            report[0] |= 1 << 1;
        }
        _ => {
            report[0] |= 1 << 5;
            report[0] |= 1 << 3;
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::{crc16_ccitt_false, encoder::PacketAcV6PacketEncoder, set_usb_read_flag};

    #[test]
    fn set_usb_read_flag_sets_ac_control_byte_bit_and_refreshes_crc() {
        let mut encoder = PacketAcV6PacketEncoder::new();
        let mut packet = encoder.encode_compact_report(&[0; 8]).to_vec();

        set_usb_read_flag(&mut packet).expect("should set USB_READ flag");

        assert_eq!(packet[3] & 0x40, 0);
        assert_eq!(packet[30] & (1 << 5), 1 << 5);
        assert_eq!(read_u16_le(&packet, 37), crc16_ccitt_false(&packet[..37]));
    }

    #[test]
    fn set_usb_read_flag_sets_m_control_byte_bit_and_refreshes_crc() {
        let mut packet = vec![0u8; 19];
        packet[0] = b'M';
        let crc = crc16_ccitt_false(&packet[..17]);
        packet[17..].copy_from_slice(&crc.to_le_bytes());

        set_usb_read_flag(&mut packet).expect("should set USB_READ flag");

        assert_eq!(packet[16] & (1 << 5), 1 << 5);
        assert_eq!(read_u16_le(&packet, 17), crc16_ccitt_false(&packet[..17]));
    }

    fn read_u16_le(packet: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([packet[offset], packet[offset + 1]])
    }
}
