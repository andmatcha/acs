pub(crate) const PACKET_ACV6_PACKET_LEN: usize = 39;
pub(crate) const PACKET_ACV6_PAYLOAD_LEN: usize = 37;
pub(crate) const PACKET_ACV6_MANUAL_MODE_VALUE: u8 = 1;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketAcV6Thresholds {
    pub trigger: u8,
    pub stick_low: u8,
    pub stick_high: u8,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketAcV6ProfileDefinition {
    pub base_horizon_positive: u16,
    pub base_horizon_negative: u16,
    pub base_roll_positive: u16,
    pub base_roll_negative: u16,
    pub pitch1_down: u16,
    pub pitch1_up: u16,
    pub pitch2_down: u16,
    pub pitch2_up: u16,
    pub pitch3_up: u16,
    pub pitch3_down: u16,
    pub roll_positive: u16,
    pub roll_negative: u16,
    pub gripper_close: u16,
    pub gripper_open: u16,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketAcV6Definition {
    pub header: [u8; 2],
    pub neutral_current: u16,
    pub thresholds: PacketAcV6Thresholds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketAcV6Profile {
    Normal,
    Power,
    Sensitive,
}

pub(crate) const PACKET_ACV6_DEFINITION: PacketAcV6Definition = PacketAcV6Definition {
    header: *b"AC",
    neutral_current: 255,
    thresholds: PacketAcV6Thresholds {
        trigger: 205,
        stick_low: 25,
        stick_high: 230,
    },
};

const PACKET_ACV6_PROFILE_NORMAL: PacketAcV6ProfileDefinition = PacketAcV6ProfileDefinition {
    base_horizon_positive: 155,
    base_horizon_negative: 355,
    base_roll_positive: 315,
    base_roll_negative: 205,
    pitch1_down: 230,
    pitch1_up: 280,
    pitch2_down: 225,
    pitch2_up: 275,
    pitch3_up: 210,
    pitch3_down: 400,
    roll_positive: 190,
    roll_negative: 310,
    gripper_close: 155,
    gripper_open: 285,
};

const PACKET_ACV6_PROFILE_POWER: PacketAcV6ProfileDefinition = PacketAcV6ProfileDefinition {
    base_horizon_positive: 100,
    base_horizon_negative: 400,
    base_roll_positive: 511,
    base_roll_negative: 1,
    pitch1_down: 170,
    pitch1_up: 340,
    pitch2_down: 175,
    pitch2_up: 325,
    pitch3_up: 160,
    pitch3_down: 450,
    roll_positive: 80,
    roll_negative: 430,
    gripper_close: 105,
    gripper_open: 335,
};

const PACKET_ACV6_PROFILE_SENSITIVE: PacketAcV6ProfileDefinition = PacketAcV6ProfileDefinition {
    base_horizon_positive: 180,
    base_horizon_negative: 330,
    base_roll_positive: 295,
    base_roll_negative: 225,
    pitch1_down: 230,
    pitch1_up: 260,
    pitch2_down: 215,
    pitch2_up: 280,
    pitch3_up: 225,
    pitch3_down: 290,
    roll_positive: 210,
    roll_negative: 300,
    gripper_close: 240,
    gripper_open: 270,
};

impl PacketAcV6Profile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Power => "power",
            Self::Sensitive => "sensitive",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Normal => Self::Power,
            Self::Power => Self::Sensitive,
            Self::Sensitive => Self::Normal,
        }
    }

    pub(crate) fn definition(self) -> &'static PacketAcV6ProfileDefinition {
        match self {
            Self::Normal => &PACKET_ACV6_PROFILE_NORMAL,
            Self::Power => &PACKET_ACV6_PROFILE_POWER,
            Self::Sensitive => &PACKET_ACV6_PROFILE_SENSITIVE,
        }
    }
}
