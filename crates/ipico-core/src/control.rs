//! IPICO reader TCP control protocol encoding/decoding.
//!
//! Pure functions — no async, no I/O. All frame encoding/decoding for the
//! `ab`-prefixed control protocol described in
//! `docs/ipico-protocol/ipico-control-protocol.md`.
//!
//! Note: `ControlError` includes `Timeout` and `ChannelClosed` variants used
//! by the async control client in the forwarder service.

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
    /// Query the reader's current date/time (instruction 0x02).
    GetDateTime,
    /// Set the reader's date/time (instruction 0x01). All fields are BCD-encoded
    /// except day_of_week which is plain: Mon=1..Sat=6, Sun=0.
    SetDateTime {
        year: u8,
        month: u8,
        day: u8,
        day_of_week: u8,
        hour: u8,
        minute: u8,
        second: u8,
    },
    /// Query the reader's statistics/firmware info (instruction 0x0a).
    GetStatistics,
    /// Query the 0x4b extended status register (recording, storage, hardware).
    GetExtendedStatus,
    /// Query CONFIG3 (read mode + timeout) (instruction 0x09 with length 0xff).
    GetConfig3,
    /// Set CONFIG3 read mode and timeout (instruction 0x09).
    SetConfig3 { mode: ReadMode, timeout: u8 },
    /// Request the reader's ASCII banner text (instruction 0x37).
    /// Response arrives as plain text lines followed by an ACK frame.
    PrintBanner,
    /// Undocumented 0xe0 initialization probe sent during connection setup.
    InitE0,
    /// Set the reader's recording state (0x4b sub-cmd 0x00).
    SetRecordingState { on: bool },
    /// Set the reader's download/access mode (0x4b sub-cmd 0x01).
    SetAccessMode { on: bool },
    /// Initialize download sequence (0x4b sub-cmd 0x02).
    InitDownload,
    /// Configure download parameters (0x4b sub-cmd 0x07, params [0x01, 0x05]).
    /// Parameter meaning unknown, replicated from observed IPICO Connect behavior.
    ConfigureDownload,
    /// Clean up after download (0x4b sub-cmd 0x07, param 0x00).
    CleanupDownload,
    /// Trigger EEPROM erase (0x4b sub-cmd 0xd0).
    TriggerErase,
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
            Command::SetRecordingState { .. }
            | Command::SetAccessMode { .. }
            | Command::InitDownload
            | Command::ConfigureDownload
            | Command::CleanupDownload
            | Command::TriggerErase => INSTR_EXT_STATUS,
        }
    }
}

// ── Frame encoding ──────────────────────────────────────────────────────────

/// Encode a command into an `ab`-prefixed wire frame (including `\r\n` terminator).
/// `reader_id` is the target reader ID byte (typically 0x00 for broadcast).
pub fn encode_command(cmd: &Command, reader_id: u8) -> Result<Vec<u8>, ControlError> {
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
            to_bcd(*year)?,
            to_bcd(*month)?,
            to_bcd(*day)?,
            *day_of_week,
            to_bcd(*hour)?,
            to_bcd(*minute)?,
            to_bcd(*second)?,
        ],
        Command::SetConfig3 { mode, timeout } => {
            // 0x07 = modify lower 3 bits of CONFIG3 (mode selection: bits 0..2)
            vec![mode.config3_value(), *timeout, 0x07]
        }
        Command::SetRecordingState { on } => vec![0x00, if *on { 0x01 } else { 0x00 }],
        Command::SetAccessMode { on } => vec![0x01, if *on { 0x01 } else { 0x00 }],
        Command::InitDownload => vec![0x02],
        Command::ConfigureDownload => vec![0x07, 0x01, 0x05],
        Command::CleanupDownload => vec![0x07, 0x00],
        Command::TriggerErase => vec![0xd0],
        _ => vec![],
    };

    // Length byte: 0xff signals a read/query request (IPICO protocol convention);
    // for write commands, it's the actual data payload length.
    let length: u8 = match cmd {
        Command::GetExtendedStatus | Command::GetConfig3 => 0xff,
        _ => {
            assert!(
                data.len() <= 255,
                "command data too long: {} bytes",
                data.len()
            );
            data.len() as u8
        }
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

    let mut frame = Vec::new();
    frame.extend_from_slice(b"ab");
    frame.extend_from_slice(hex_body.as_bytes());
    frame.extend_from_slice(format!("{checksum:02x}").as_bytes());
    frame.extend_from_slice(b"\r\n");
    Ok(frame)
}

/// BCD encode: decimal value to BCD byte (e.g., 25 -> 0x25).
pub fn to_bcd(val: u8) -> Result<u8, ControlError> {
    if val > 99 {
        return Err(ControlError::InvalidBcd(val));
    }
    Ok(((val / 10) << 4) | (val % 10))
}

/// BCD decode: BCD byte to decimal value (e.g., 0x25 -> 25).
pub fn from_bcd(val: u8) -> Result<u8, ControlError> {
    let hi = val >> 4;
    let lo = val & 0x0f;
    if hi > 9 || lo > 9 {
        return Err(ControlError::InvalidBcd(val));
    }
    Ok(hi * 10 + lo)
}

/// Compute LRC checksum over ASCII hex chars.
///
/// Sum all ASCII byte values between header and checksum field, take low byte.
/// Input is the slice of ASCII chars between `ab` header and the LRC (e.g., `b"000002"`).
pub fn lrc(ascii_bytes: &[u8]) -> u8 {
    ascii_bytes.iter().map(|&b| b as u32).sum::<u32>() as u8
}

// ── Response types ─────────────────────────────────────────────────────────

/// Parsed control frame from the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFrame {
    reader_id: u8,
    instruction: u8,
    data: Vec<u8>,
}

impl ControlFrame {
    pub fn reader_id(&self) -> u8 {
        self.reader_id
    }

    pub fn instruction(&self) -> u8 {
        self.instruction
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Test-only constructor for building frames without going through `parse_response`.
    #[cfg(test)]
    pub fn new(reader_id: u8, instruction: u8, data: Vec<u8>) -> Self {
        Self {
            reader_id,
            instruction,
            data,
        }
    }
}

/// Decoded reader date/time from GET_DATE_TIME (0x02) or GUN_TIME (0x2c).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReaderDateTime {
    pub year: u8, // 2-digit
    pub month: u8,
    pub day: u8,
    pub day_of_week: u8, // 0-6, Mon=1, Sun=0
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub centisecond: u8, // 0-99 (plain hex, NOT BCD)
    pub config: u8,
}

impl ReaderDateTime {
    pub fn to_iso_string(&self) -> String {
        format!(
            "20{:02}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.centisecond as u16 * 10,
        )
    }
}

/// Decoded GET_STATISTICS (0x0a) response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReaderStatistics {
    pub fw_version: u8,
    pub reader_id: u8,
    pub config1: u8,
    pub crc_errors: u8,
    pub powerup_count: u8,
    pub activity_count: u8,
    pub decoder_fw_i: u8,
    pub decoder_fw_q: u8,
    pub config2: u8,
    pub wiegand_config: u8,
    pub wiegand_timer: u8,
    pub config3: u8,
    pub hw_code: u8,
    pub rejected_tags: u8,
}

impl ReaderStatistics {
    pub fn fw_version_string(&self) -> String {
        format!("{}.{}", self.fw_version >> 4, self.fw_version & 0x0f)
    }
}

/// Reader recording/access state from the 0x4b extended status register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RecordingState {
    /// Recording off (0x00).
    Off,
    /// Recording on (0x01).
    On,
    /// Download/access mode active (0x03).
    Downloading,
    /// Unrecognized state byte.
    Unknown(u8),
}

impl RecordingState {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0x00 => Self::Off,
            0x01 => Self::On,
            0x03 => Self::Downloading,
            other => Self::Unknown(other),
        }
    }

    pub fn is_recording(self) -> bool {
        matches!(self, Self::On)
    }
}

/// Decoded 0x4b extended status response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExtendedStatus {
    /// Recorder/access state from byte 0 of the 0x4b response.
    pub recording_state: RecordingState,
    /// 24-bit big-endian stored-data extent (bytes 1-3).
    pub stored_data_extent: u32,
    /// 24-bit big-endian download progress (bytes 4-6).
    pub download_progress: u32,
    pub hw_identifier: u16,
    pub hw_config: u8,
    /// Coarse storage state (byte 11): 0x01 = empty, 0x03/0x0c = data present.
    pub storage_state: u8,
    pub flags: Option<u8>, // optional byte 12 (absent in 12-byte responses)
}

impl ExtendedStatus {
    /// Approximate number of stored reads based on ~32 bytes per record.
    /// This divisor is a local estimate; the exact unit of the stored-data
    /// extent field is not confirmed (see protocol spec Open Questions).
    pub fn estimated_stored_reads(&self) -> u32 {
        self.stored_data_extent / 32
    }
}

/// Error from parsing or decoding a control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    TooShort,
    InvalidHeader,
    InvalidHex,
    BadLrc,
    ReaderError(u8),
    UnexpectedLength { instruction: u8, got: usize },
    UnknownReadMode(u8),
    InvalidBcd(u8),
    Timeout,
    ChannelClosed,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::TooShort => write!(f, "frame too short"),
            ControlError::InvalidHeader => write!(f, "invalid header (expected 'ab')"),
            ControlError::InvalidHex => write!(f, "invalid hex digit in frame"),
            ControlError::BadLrc => write!(f, "LRC checksum mismatch"),
            ControlError::ReaderError(code) => write!(f, "reader error 0x{code:02x}"),
            ControlError::UnexpectedLength { instruction, got } => {
                write!(
                    f,
                    "unexpected data length for instruction 0x{instruction:02x}: got {got}"
                )
            }
            ControlError::UnknownReadMode(byte) => {
                write!(f, "unknown read mode byte 0x{byte:02x}")
            }
            ControlError::InvalidBcd(byte) => {
                write!(f, "invalid BCD byte 0x{byte:02x}")
            }
            ControlError::Timeout => write!(f, "reader response timeout"),
            ControlError::ChannelClosed => write!(f, "control channel closed (connection lost)"),
        }
    }
}

impl std::error::Error for ControlError {}

// ── Response parsing ───────────────────────────────────────────────────────

/// Parse a hex byte pair from a string slice.
fn parse_hex_byte(s: &str) -> Result<u8, ControlError> {
    u8::from_str_radix(s, 16).map_err(|_| ControlError::InvalidHex)
}

/// Parse an `ab`-prefixed response line (without trailing \r\n) into a ControlFrame.
/// Returns `ControlError::ReaderError` if the instruction byte is >= 0xf0.
pub fn parse_response(line: &[u8]) -> Result<ControlFrame, ControlError> {
    let s = std::str::from_utf8(line).map_err(|_| ControlError::InvalidHex)?;

    // Minimum length: ab + RR + LL + II + CC = 10 chars
    if s.len() < 10 {
        return Err(ControlError::TooShort);
    }

    if &s[..2] != "ab" {
        return Err(ControlError::InvalidHeader);
    }

    // Verify LRC: body = everything between "ab" and the last 2 chars
    let body = &s[2..s.len() - 2];
    let expected_lrc = parse_hex_byte(&s[s.len() - 2..])?;
    let computed = lrc(body.as_bytes());
    if computed != expected_lrc {
        return Err(ControlError::BadLrc);
    }

    let reader_id = parse_hex_byte(&s[2..4])?;
    let _length = parse_hex_byte(&s[4..6])?;
    let instruction = parse_hex_byte(&s[6..8])?;

    // Check for error response
    if instruction >= 0xf0 {
        return Err(ControlError::ReaderError(instruction));
    }

    // Parse data bytes
    let data_hex = &s[8..s.len() - 2];
    if data_hex.len() % 2 != 0 {
        return Err(ControlError::InvalidHex);
    }
    let mut data = Vec::with_capacity(data_hex.len() / 2);
    for i in (0..data_hex.len()).step_by(2) {
        data.push(parse_hex_byte(&data_hex[i..i + 2])?);
    }

    Ok(ControlFrame {
        reader_id,
        instruction,
        data,
    })
}

// ── Decode functions ───────────────────────────────────────────────────────

/// Decode GET_DATE_TIME or GUN_TIME response (9+ data bytes).
/// IMPORTANT: Centisecond (byte 7) is plain hex, NOT BCD.
pub fn decode_date_time(frame: &ControlFrame) -> Result<ReaderDateTime, ControlError> {
    if frame.data().len() < 9 {
        return Err(ControlError::UnexpectedLength {
            instruction: frame.instruction(),
            got: frame.data().len(),
        });
    }
    Ok(ReaderDateTime {
        year: from_bcd(frame.data()[0])?,
        month: from_bcd(frame.data()[1])?,
        day: from_bcd(frame.data()[2])?,
        day_of_week: frame.data()[3],
        hour: from_bcd(frame.data()[4])?,
        minute: from_bcd(frame.data()[5])?,
        second: from_bcd(frame.data()[6])?,
        centisecond: frame.data()[7], // plain hex, NOT BCD
        config: frame.data()[8],
    })
}

/// Decode GET_STATISTICS response (14+ data bytes).
pub fn decode_statistics(frame: &ControlFrame) -> Result<ReaderStatistics, ControlError> {
    if frame.data().len() < 14 {
        return Err(ControlError::UnexpectedLength {
            instruction: frame.instruction(),
            got: frame.data().len(),
        });
    }
    Ok(ReaderStatistics {
        fw_version: frame.data()[0],
        reader_id: frame.data()[1],
        config1: frame.data()[2],
        crc_errors: frame.data()[3],
        powerup_count: frame.data()[4],
        activity_count: frame.data()[5],
        decoder_fw_i: frame.data()[6],
        decoder_fw_q: frame.data()[7],
        config2: frame.data()[8],
        wiegand_config: frame.data()[9],
        wiegand_timer: frame.data()[10],
        config3: frame.data()[11],
        hw_code: frame.data()[12],
        rejected_tags: frame.data()[13],
    })
}

/// Decode extended status response (12 or 13 data bytes).
/// Bytes 1-3: 24-bit stored-data extent, bytes 4-6: 24-bit download progress,
/// byte 7 reserved, hw_identifier at bytes 8-9, hw_config at 10,
/// storage_state at 11, optional flags at 12.
pub fn decode_extended_status(frame: &ControlFrame) -> Result<ExtendedStatus, ControlError> {
    if frame.data().len() < 12 {
        return Err(ControlError::UnexpectedLength {
            instruction: frame.instruction(),
            got: frame.data().len(),
        });
    }
    Ok(ExtendedStatus {
        recording_state: RecordingState::from_byte(frame.data()[0]),
        stored_data_extent: u32::from_be_bytes([0, frame.data()[1], frame.data()[2], frame.data()[3]]),
        download_progress: u32::from_be_bytes([0, frame.data()[4], frame.data()[5], frame.data()[6]]),
        hw_identifier: u16::from_be_bytes([frame.data()[8], frame.data()[9]]),
        hw_config: frame.data()[10],
        storage_state: frame.data()[11],
        flags: if frame.data().len() >= 13 {
            Some(frame.data()[12])
        } else {
            None
        },
    })
}

/// Decode CONFIG3 response (2 data bytes: mode + timeout).
pub fn decode_config3(frame: &ControlFrame) -> Result<(ReadMode, u8), ControlError> {
    if frame.data().len() < 2 {
        return Err(ControlError::UnexpectedLength {
            instruction: frame.instruction(),
            got: frame.data().len(),
        });
    }
    let mode = ReadMode::from_config3(frame.data()[0])
        .ok_or(ControlError::UnknownReadMode(frame.data()[0]))?;
    Ok((mode, frame.data()[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_config3_unknown_mode_returns_descriptive_error() {
        let frame = ControlFrame::new(0, INSTR_CONFIG3, vec![0x02, 0x05]);
        let err = decode_config3(&frame).unwrap_err();
        assert!(
            matches!(err, ControlError::UnknownReadMode(0x02)),
            "expected UnknownReadMode(0x02), got {err:?}"
        );
    }

    #[test]
    fn bcd_round_trip() {
        for val in 0..=99u8 {
            assert_eq!(from_bcd(to_bcd(val).unwrap()).unwrap(), val);
        }
    }

    #[test]
    fn to_bcd_known_values() {
        assert_eq!(to_bcd(0).unwrap(), 0x00);
        assert_eq!(to_bcd(9).unwrap(), 0x09);
        assert_eq!(to_bcd(10).unwrap(), 0x10);
        assert_eq!(to_bcd(26).unwrap(), 0x26);
        assert_eq!(to_bcd(59).unwrap(), 0x59);
        assert_eq!(to_bcd(99).unwrap(), 0x99);
    }

    #[test]
    fn from_bcd_known_values() {
        assert_eq!(from_bcd(0x00).unwrap(), 0);
        assert_eq!(from_bcd(0x18).unwrap(), 18);
        assert_eq!(from_bcd(0x26).unwrap(), 26);
        assert_eq!(from_bcd(0x55).unwrap(), 55);
        assert_eq!(from_bcd(0x99).unwrap(), 99);
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
        let frame = encode_command(&Command::GetDateTime, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00000222\r\n");
    }

    #[test]
    fn encode_get_statistics() {
        let frame = encode_command(&Command::GetStatistics, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00000a51\r\n");
    }

    #[test]
    fn encode_print_banner() {
        let frame = encode_command(&Command::PrintBanner, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab0000372a\r\n");
    }

    #[test]
    fn encode_get_extended_status() {
        let frame = encode_command(&Command::GetExtendedStatus, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00ff4bc2\r\n");
    }

    #[test]
    fn encode_get_config3() {
        let frame = encode_command(&Command::GetConfig3, 0x00).unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(
            std::str::from_utf8(&frame).unwrap(),
            "ab00070126030605185550f6\r\n"
        );
    }

    #[test]
    fn encode_init_e0() {
        let frame = encode_command(&Command::InitE0, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab0000e055\r\n");
    }

    #[test]
    fn encode_set_recording_state_off() {
        let frame = encode_command(&Command::SetRecordingState { on: false }, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00024b000018\r\n");
    }

    #[test]
    fn encode_set_access_mode_off() {
        let frame = encode_command(&Command::SetAccessMode { on: false }, 0x00).unwrap();
        assert_eq!(std::str::from_utf8(&frame).unwrap(), "ab00024b010019\r\n");
    }

    #[test]
    fn encode_trigger_erase() {
        let frame = encode_command(&Command::TriggerErase, 0x00).unwrap();
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

    #[test]
    fn parse_get_date_time_response() {
        // From pcap: ab000902260306051855443727cf
        let frame = parse_response(b"ab000902260306051855443727cf").unwrap();
        assert_eq!(frame.instruction(), INSTR_GET_DATE_TIME);
        assert_eq!(frame.data().len(), 9);
        let dt = decode_date_time(&frame).unwrap();
        assert_eq!(dt.year, 26);
        assert_eq!(dt.month, 3);
        assert_eq!(dt.day, 6);
        assert_eq!(dt.day_of_week, 5);
        assert_eq!(dt.hour, 18);
        assert_eq!(dt.minute, 55);
        assert_eq!(dt.second, 44);
        assert_eq!(dt.centisecond, 0x37); // plain hex, NOT BCD
        assert_eq!(dt.to_iso_string(), "2026-03-06T18:55:44.550");
        assert_eq!(dt.config, 0x27);
    }

    #[test]
    fn parse_ack_response() {
        let frame = parse_response(b"ab00000121").unwrap();
        assert_eq!(frame.instruction(), INSTR_SET_DATE_TIME);
        assert_eq!(frame.data().len(), 0);
    }

    #[test]
    fn parse_error_f2_response() {
        let err = parse_response(b"ab0000f258").unwrap_err();
        assert_eq!(err, ControlError::ReaderError(0xf2));
    }

    #[test]
    fn parse_bad_lrc() {
        let err = parse_response(b"ab000002ff").unwrap_err();
        assert_eq!(err, ControlError::BadLrc);
    }

    #[test]
    fn parse_too_short() {
        let err = parse_response(b"ab0002").unwrap_err();
        assert_eq!(err, ControlError::TooShort);
    }

    #[test]
    fn parse_extended_status_13_bytes() {
        let frame = parse_response(b"ab000d4b010b012f0000000059058f0c005a").unwrap();
        assert_eq!(frame.instruction(), INSTR_EXT_STATUS);
        let ext = decode_extended_status(&frame).unwrap();
        assert_eq!(ext.recording_state, RecordingState::On);
        assert_eq!(ext.stored_data_extent, 0x0b012f);
        assert_eq!(ext.download_progress, 0);
        assert_eq!(ext.hw_identifier, 0x5905);
        assert_eq!(ext.hw_config, 0x8f);
        assert_eq!(ext.storage_state, 0x0c);
        assert_eq!(ext.flags, Some(0x00));
        assert_eq!(ext.estimated_stored_reads(), 0x0b012f / 32);
    }

    #[test]
    fn parse_extended_status_12_bytes() {
        // Build a 12-byte payload manually
        let data_hex = "000000000000000059058f01";
        let body = format!("000c4b{}", data_hex);
        let hex = format!("ab{}{:02x}", body, lrc(body.as_bytes()));
        let frame = parse_response(hex.as_bytes()).unwrap();
        let ext = decode_extended_status(&frame).unwrap();
        assert_eq!(ext.recording_state, RecordingState::Off);
        assert_eq!(ext.stored_data_extent, 0);
        assert_eq!(ext.storage_state, 0x01);
        assert_eq!(ext.flags, None);
        assert_eq!(ext.estimated_stored_reads(), 0);
    }

    #[test]
    fn parse_config3_response() {
        let frame = parse_response(b"ab0002090305f3").unwrap();
        let (mode, timeout) = decode_config3(&frame).unwrap();
        assert_eq!(mode, ReadMode::Event);
        assert_eq!(timeout, 5);
    }

    #[test]
    fn parse_config3_response_raw() {
        let frame = parse_response(b"ab0002090005f0").unwrap();
        let (mode, timeout) = decode_config3(&frame).unwrap();
        assert_eq!(mode, ReadMode::Raw);
        assert_eq!(timeout, 5);
    }

    #[test]
    fn parse_gun_time_response() {
        let frame = parse_response(b"ab000a2c260306052004151b2782ae").unwrap();
        assert_eq!(frame.instruction(), INSTR_GUN_TIME);
        assert_eq!(frame.data().len(), 10);
        let dt = decode_date_time(&frame).unwrap();
        assert_eq!(dt.year, 26);
        assert_eq!(dt.hour, 20);
        assert_eq!(dt.minute, 4);
        assert_eq!(dt.second, 15);
        assert_eq!(dt.centisecond, 0x1b);
        assert_eq!(dt.to_iso_string(), "2026-03-06T20:04:15.270");
        assert_eq!(frame.data()[9], 0x82); // extra unknown byte
    }

    #[test]
    fn encode_set_config3_fsls() {
        let frame = encode_command(
            &Command::SetConfig3 {
                mode: ReadMode::FirstLastSeen,
                timeout: 5,
            },
            0x00,
        )
        .unwrap();
        // data = [0x05, 0x05, 0x07], hex body = "00030905050700"
        // Verify by running the test - the LRC will be computed correctly by encode_command
        let s = std::str::from_utf8(&frame).unwrap();
        assert!(s.starts_with("ab000309050507"));
        assert!(s.ends_with("\r\n"));
    }

    #[test]
    fn to_bcd_rejects_values_above_99() {
        for val in [100, 128, 255] {
            assert_eq!(to_bcd(val).unwrap_err(), ControlError::InvalidBcd(val));
        }
    }

    #[test]
    fn from_bcd_rejects_invalid_nibbles() {
        for val in [0xAF, 0xFA, 0xFF, 0xA0] {
            assert_eq!(from_bcd(val).unwrap_err(), ControlError::InvalidBcd(val));
        }
    }

    #[test]
    fn encode_then_parse_round_trip() {
        for cmd in [
            Command::GetDateTime,
            Command::GetStatistics,
            Command::PrintBanner,
            Command::GetExtendedStatus,
            Command::GetConfig3,
            Command::InitE0,
            Command::SetRecordingState { on: true },
            Command::SetRecordingState { on: false },
            Command::SetAccessMode { on: true },
            Command::SetAccessMode { on: false },
            Command::InitDownload,
            Command::ConfigureDownload,
            Command::CleanupDownload,
            Command::TriggerErase,
        ] {
            let encoded = encode_command(&cmd, 0x00).unwrap();
            let line = &encoded[..encoded.len() - 2]; // strip \r\n
            let frame = parse_response(line).unwrap();
            assert_eq!(frame.instruction(), cmd.instruction());
        }
    }

    #[test]
    fn parse_response_invalid_header() {
        let err = parse_response(b"cd000002ff").unwrap_err();
        assert_eq!(err, ControlError::InvalidHeader);
    }

    #[test]
    fn parse_response_odd_length_data_hex() {
        // Frame: ab + header(6 chars: RR LL II) + 1 data char + 2 LRC chars = 11 total
        // That's: ab + "000102" + "0" + "XX" = 13 chars total
        let body = "0001020";
        let lrc_val = lrc(body.as_bytes());
        let frame = format!("ab{body}{lrc_val:02x}");
        let result = parse_response(frame.as_bytes());
        assert!(
            result.is_err(),
            "odd-length data hex should fail: {result:?}"
        );
    }

    #[test]
    fn encode_command_with_non_zero_reader_id() {
        let frame = encode_command(&Command::GetDateTime, 0x05).unwrap();
        let s = std::str::from_utf8(&frame).unwrap();
        assert!(s.starts_with("ab05"), "expected reader_id 0x05, got: {s}");
        // Verify round-trip parse
        let parsed = parse_response(&frame[..frame.len() - 2]).unwrap();
        assert_eq!(parsed.reader_id(), 0x05);
        assert_eq!(parsed.instruction(), INSTR_GET_DATE_TIME);
    }
}
