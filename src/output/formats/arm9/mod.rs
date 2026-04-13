mod encoder;
mod sound;

use crate::input::compact::CompactReport;
use crate::output::formats::{OutputDriver, OutputFormat};
use encoder::ManualPacketEncoder;
use sound::ModeSoundPlayer;

pub(crate) fn create_driver() -> Box<dyn OutputDriver> {
    Box::new(Arm9OutputDriver::new())
}

struct Arm9OutputDriver {
    encoder: ManualPacketEncoder,
    sound_player: ModeSoundPlayer,
}

impl Arm9OutputDriver {
    fn new() -> Self {
        Self {
            encoder: ManualPacketEncoder::new(),
            sound_player: ModeSoundPlayer::new(),
        }
    }
}

impl OutputDriver for Arm9OutputDriver {
    fn format_name(&self) -> &'static str {
        OutputFormat::Arm9.as_str()
    }

    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String> {
        let update = self.encoder.encode_compact_report_update(compact_report);
        if update.profile_changed {
            self.sound_player.play(update.profile.as_str());
        }
        Ok(update.packet.to_vec())
    }
}
