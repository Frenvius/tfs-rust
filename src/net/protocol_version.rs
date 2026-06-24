use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClientVersion {
    V860,
    V1098,
}

impl ClientVersion {
    pub fn min_version(self) -> u16 {
        match self {
            Self::V860 => 860,
            Self::V1098 => 1097,
        }
    }

    pub fn max_version(self) -> u16 {
        match self {
            Self::V860 => 860,
            Self::V1098 => 1098,
        }
    }

    pub fn is_1098(self) -> bool {
        self == Self::V1098
    }
}

static VERSION: OnceLock<ClientVersion> = OnceLock::new();

pub fn client_version() -> ClientVersion {
    *VERSION.get().expect("client version not initialized")
}

pub fn init_client_version(min: u16) {
    let v = if min >= 1097 {
        ClientVersion::V1098
    } else {
        ClientVersion::V860
    };
    VERSION
        .set(v)
        .unwrap_or_else(|_| panic!("client version already initialized"));
}

pub fn max_known_creatures() -> usize {
    match client_version() {
        ClientVersion::V860 => 1300,
        ClientVersion::V1098 => 1300,
    }
}

pub fn translate_speak_class_to_client(internal: u8) -> u8 {
    if !client_version().is_1098() {
        return internal;
    }
    match internal {
        1 => 1,   // SAY
        2 => 2,   // WHISPER
        3 => 3,   // YELL
        4 => 12,  // PRIVATE_PN -> 10.98 PRIVATE_PN
        5 => 10,  // PRIVATE_NP -> 10.98 PRIVATE_NP
        6 => 5,   // PRIVATE -> 10.98 PRIVATE_TO
        7 => 7,   // CHANNEL_Y
        8 => 6,   // CHANNEL_W -> 10.98 CHANNEL_M
        12 => 13, // BROADCAST
        13 => 14, // CHANNEL_R1
        14 => 16, // PRIVATE_RED -> 10.98 PRIVATE_RED_TO
        15 => 8,  // CHANNEL_O
        19 => 36, // MONSTER_SAY
        20 => 37, // MONSTER_YELL
        _ => internal,
    }
}

pub fn translate_speak_class_from_client(wire: u8) -> u8 {
    if !client_version().is_1098() {
        return wire;
    }
    match wire {
        1 => 1,   // SAY
        2 => 2,   // WHISPER
        3 => 3,   // YELL
        4 => 6,   // PRIVATE_FROM -> internal PRIVATE
        5 => 6,   // PRIVATE_TO -> internal PRIVATE
        6 => 8,   // CHANNEL_M -> internal CHANNEL_W
        7 => 7,   // CHANNEL_Y
        8 => 15,  // CHANNEL_O
        10 => 5,  // PRIVATE_NP
        12 => 4,  // PRIVATE_PN
        13 => 12, // BROADCAST
        14 => 13, // CHANNEL_R1
        15 => 14, // PRIVATE_RED_FROM -> internal PRIVATE_RED
        16 => 14, // PRIVATE_RED_TO -> internal PRIVATE_RED
        36 => 19, // MONSTER_SAY
        37 => 20, // MONSTER_YELL
        _ => wire,
    }
}

pub fn translate_message_class_to_client(internal: u8) -> u8 {
    if !client_version().is_1098() {
        return internal;
    }
    // Internal values are 8.60 wire values; remap to 10.98 wire values.
    // 8.60 internal -> 10.98 wire:
    match internal {
        18 => 18, // STATUS_CONSOLE_RED -> 10.98 MESSAGE_STATUS_WARNING (close enough)
        19 => 36, // EVENT_ORANGE
        20 => 17, // STATUS_DEFAULT
        21 => 22, // INFO_DESCR
        22 => 19, // EVENT_ADVANCE
        23 => 17, // EVENT_DEFAULT -> STATUS_DEFAULT
        24 => 17, // STATUS_DEFAULT (alt)
        25 => 22, // INFO_DESCR (alt)
        26 => 21, // STATUS_SMALL
        27 => 18, // STATUS_CONSOLE_BLUE -> STATUS_WARNING
        _ => internal,
    }
}

// 8.60 MESSAGE_STATUS_SMALL wire value (used in send_status_message_to_player)
pub fn message_status_small() -> u8 {
    if client_version().is_1098() { 21 } else { 0x04 }
}
