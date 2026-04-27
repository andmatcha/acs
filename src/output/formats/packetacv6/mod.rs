mod definition;
mod encoder;
mod reduced;
mod sound;

use crate::input::compact::CompactReport;
use crate::output::formats::{DummyPayloadGenerator, OutputDriver};
use encoder::PacketAcV6PacketEncoder;
use reduced::ReducedAcPacketKind;
use sound::ModeSoundPlayer;

pub(crate) fn create_driver() -> Box<dyn OutputDriver> {
    Box::new(PacketAcV6OutputDriver::new())
}

pub(crate) const fn packet_len() -> usize {
    crate::output::formats::packetacv6::definition::PACKET_ACV6_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketAcV6DummyGenerator::default()))
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
