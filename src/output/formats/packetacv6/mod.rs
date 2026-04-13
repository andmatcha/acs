mod definition;
mod encoder;
mod sound;

use crate::input::compact::CompactReport;
use crate::output::formats::OutputDriver;
use encoder::PacketAcV6PacketEncoder;
use sound::ModeSoundPlayer;

pub(crate) fn create_driver() -> Box<dyn OutputDriver> {
    Box::new(PacketAcV6OutputDriver::new())
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
