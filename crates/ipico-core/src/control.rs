//! IPICO reader TCP control protocol encoding/decoding.
//!
//! Pure functions — no async, no I/O. All frame encoding/decoding for the
//! `ab`-prefixed control protocol described in `docs/ipico-control-protocol.md`.

use std::fmt;

// ── Instruction byte constants ──────────────────────────────────────────────

pub const INSTR_SET_DATE_TIME: u8 = 0x01;
pub const INSTR_GET_DATE_TIME: u8 = 0x02;
pub const INSTR_CONFIG3: u8 = 0x09;
pub const INSTR_GET_STATISTICS: u8 = 0x0a;
pub const INSTR_GUN_TIME: u8 = 0x2c;
pub const INSTR_PRINT_BANNER: u8 = 0x37;
pub const INSTR_EXT_STATUS: u8 = 0x4b;
pub const INSTR_UNSOLICITED_STATUS: u8 = 0x4c;
pub const INSTR_UNKNOWN_E0: u8 = 0xe0;

// ── ReadMode ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    Raw,
    Event,
    FirstLastSeen,
}

impl ReadMode {
    pub fn config3_value(self) -> u8 {
        match self {
            ReadMode::Raw => 0x00,
            ReadMode::Event => 0x03,
            ReadMode::FirstLastSeen => 0x05,
        }
    }

    pub fn from_config3(byte: u8) -> Option<ReadMode> {
        match byte & 0x07 {
            0x00 => Some(ReadMode::Raw),
            0x03 => Some(ReadMode::Event),
            0x05 => Some(ReadMode::FirstLastSeen),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReadMode::Raw => "raw",
            ReadMode::Event => "event",
            ReadMode::FirstLastSeen => "fsls",
        }
    }
}

impl fmt::Display for ReadMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Command ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    GetDateTime,
    SetDateTime {
        year: u8,
        month: u8,
        day: u8,
        day_of_week: u8,
        hour: u8,
        minute: u8,
        second: u8,
    },
    GetStatistics,
    GetExtendedStatus,
    GetConfig3,
    SetConfig3 {
        mode: ReadMode,
        timeout: u8,
    },
    PrintBanner,
    InitE0,
    SetExtendedStatus {
        data: Vec<u8>,
    },
}

impl Command {
    pub fn instruction(&self) -> u8 {
        match self {
            Command::GetDateTime => INSTR_GET_DATE_TIME,
            Command::SetDateTime { .. } => INSTR_SET_DATE_TIME,
            Command::GetStatistics => INSTR_GET_STATISTICS,
            Command::GetExtendedStatus => INSTR_EXT_STATUS,
            Command::GetConfig3 => INSTR_CONFIG3,
            Command::SetConfig3 { .. } => INSTR_CONFIG3,
            Command::PrintBanner => INSTR_PRINT_BANNER,
            Command::InitE0 => INSTR_UNKNOWN_E0,
            Command::SetExtendedStatus { .. } => INSTR_EXT_STATUS,
        }
    }
}

// ── Frame encoding ──────────────────────────────────────────────────────────

pub fn encode_command(cmd: &Command, reader_id: u8) -> Vec<u8> {
    // Build data payload bytes
    let data: Vec<u8> = match cmd {
        Command::SetDateTime {
            year,
            month,
            day,
            day_of_week,
            hour,
            minute,
            second,
        } => vec![
            to_bcd(*year),
            to_bcd(*month),
            to_bcd(*day),
            *day_of_week,
            to_bcd(*hour),
            to_bcd(*minute),
            to_bcd(*second),
        ],
        Command::SetConfig3 { mode, timeout } => {
            vec![mode.config3_value(), *timeout, 0x07]
        }
        Command::SetExtendedStatus { data } => data.clone(),
        _ => vec![],
    };

    // Length byte: 0xff for query-mode commands, otherwise data length
    let length: u8 = match cmd {
        Command::GetExtendedStatus | Command::GetConfig3 => 0xff,
        _ => data.len() as u8,
    };

    let instr = cmd.instruction();

    // Build the hex body: RR LL II DD...
    let mut hex_body = String::new();
    hex_body.push_str(&format!("{reader_id:02x}"));
    hex_body.push_str(&format!("{length:02x}"));
    hex_body.push_str(&format!("{instr:02x}"));
    for b in &data {
        hex_body.push_str(&format!("{b:02x}"));
    }

    // Compute LRC over the hex body
    let checksum = lrc(hex_body.as_bytes());

    // Assemble full frame
    let mut frame = Vec::new();
    frame.extend_from_slice(b"ab");
    frame.extend_from_slice(hex_body.as_bytes());
    frame.extend_from_slice(format!("{checksum:02x}").as_bytes());
    frame.extend_from_slice(b"\r\n");
    frame
}

/// BCD encode: decimal value to BCD byte (e.g., 25 -> 0x25).
pub fn to_bcd(val: u8) -> u8 {
    ((val / 10) << 4) | (val % 10)
}

/// BCD decode: BCD byte to decimal value (e.g., 0x25 -> 25).
pub fn from_bcd(val: u8) -> u8 {
    (val >> 4) * 10 + (val & 0x0f)
}

/// Compute LRC checksum over ASCII hex chars.
///
/// Sum all ASCII byte values between header and checksum field, take low byte.
/// Input is the slice of ASCII chars between `ab` header and the LRC (e.g., `b"000002"`).
pub fn lrc(ascii_bytes: &[u8]) -> u8 {
    ascii_bytes.iter().map(|&b| b as u32).sum::<u32>() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd_round_trip() {
        for val in 0..=99u8 {
            assert_eq!(from_bcd(to_bcd(val)), val);
        }
    }

    #[test]
    fn to_bcd_known_values() {
        assert_eq!(to_bcd(0), 0x00);
        assert_eq!(to_bcd(9), 0x09);
        assert_eq!(to_bcd(10), 0x10);
        assert_eq!(to_bcd(26), 0x26);
        assert_eq!(to_bcd(59), 0x59);
        assert_eq!(to_bcd(99), 0x99);
    }

    #[test]
    fn from_bcd_known_values() {
        assert_eq!(from_bcd(0x00), 0);
        assert_eq!(from_bcd(0x18), 18);
        assert_eq!(from_bcd(0x26), 26);
        assert_eq!(from_bcd(0x55), 55);
        assert_eq!(from_bcd(0x99), 99);
    }

    #[test]
    fn lrc_example_from_protocol_doc() {
        // LRC("000002") = 0x30+0x30+0x30+0x30+0x30+0x32 = 0x122 -> 0x22
        assert_eq!(lrc(b"000002"), 0x22);
    }

    #[test]
    fn lrc_get_statistics_command() {
        // ab00000a -> LRC over "00000a" = 0x30*4 + 0x30 + 0x61 = 0x151 -> 0x51
        assert_eq!(lrc(b"00000a"), 0x51);
    }

    #[test]
    fn lrc_get_extended_status_command() {
        // ab00ff4b -> LRC over "00ff4b" = 0x30+0x30+0x66+0x66+0x34+0x62 = 0x1c2 -> 0xc2
        assert_eq!(lrc(b"00ff4b"), 0xc2);
    }

    #[test]
    fn encode_get_date_time() {
        let frame = encode_command(&Command::GetDateTime, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00000222\r\n");
    }

    #[test]
    fn encode_get_statistics() {
        let frame = encode_command(&Command::GetStatistics, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00000a51\r\n");
    }

    #[test]
    fn encode_print_banner() {
        let frame = encode_command(&Command::PrintBanner, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab0000372a\r\n");
    }

    #[test]
    fn encode_get_extended_status() {
        let frame = encode_command(&Command::GetExtendedStatus, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00ff4bc2\r\n");
    }

    #[test]
    fn encode_get_config3() {
        let frame = encode_command(&Command::GetConfig3, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00ff0995\r\n");
    }

    #[test]
    fn encode_set_config3_event() {
        let frame = encode_command(
            &Command::SetConfig3 {
                mode: ReadMode::Event,
                timeout: 5,
            },
            0x00,
        );
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab0003090305075b\r\n");
    }

    #[test]
    fn encode_set_config3_raw() {
        let frame = encode_command(
            &Command::SetConfig3 {
                mode: ReadMode::Raw,
                timeout: 5,
            },
            0x00,
        );
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00030900050758\r\n");
    }

    #[test]
    fn encode_set_date_time() {
        let frame = encode_command(
            &Command::SetDateTime {
                year: 26,
                month: 3,
                day: 6,
                day_of_week: 5,
                hour: 18,
                minute: 55,
                second: 50,
            },
            0x00,
        );
        assert_eq!(
            std::str::from_utf8(&frame).unwrap(),
            "ab00070126030605185550f6\r\n"
        );
    }

    #[test]
    fn encode_init_e0() {
        let frame = encode_command(&Command::InitE0, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab0000e055\r\n");
    }

    #[test]
    fn encode_set_ext_status_clear_step1() {
        let frame = encode_command(
            &Command::SetExtendedStatus {
                data: vec![0x00, 0x00],
            },
            0x00,
        );
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00024b000018\r\n");
    }

    #[test]
    fn encode_set_ext_status_clear_step2() {
        let frame = encode_command(
            &Command::SetExtendedStatus {
                data: vec![0x01, 0x00],
            },
            0x00,
        );
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00024b010019\r\n");
    }

    #[test]
    fn encode_set_ext_status_clear_trigger() {
        let frame = encode_command(&Command::SetExtendedStatus { data: vec![0xd0] }, 0x00);
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00014bd0eb\r\n");
    }

    #[test]
    fn read_mode_round_trip() {
        for mode in [ReadMode::Raw, ReadMode::Event, ReadMode::FirstLastSeen] {
            assert_eq!(ReadMode::from_config3(mode.config3_value()), Some(mode));
        }
    }

    #[test]
    fn read_mode_as_str() {
        assert_eq!(ReadMode::Raw.as_str(), "raw");
        assert_eq!(ReadMode::Event.as_str(), "event");
        assert_eq!(ReadMode::FirstLastSeen.as_str(), "fsls");
    }
}
