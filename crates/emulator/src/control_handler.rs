//! Emulated reader control state and response frame building.
//!
//! Provides `EmulatedReaderState` for tracking mutable reader state during
//! control protocol exchanges, and helpers for building valid IPICO control
//! response frames.

use ipico_core::control::{
    INSTR_CONFIG3, INSTR_EXT_STATUS, INSTR_GET_DATE_TIME, INSTR_GET_STATISTICS, INSTR_PRINT_BANNER,
    INSTR_SET_DATE_TIME, INSTR_TAG_MESSAGE_FORMAT, INSTR_UNKNOWN_E0, ReadMode, from_bcd, lrc,
    to_bcd,
};
use ipico_core::read::ReadType;

use chrono::{Datelike, Local, TimeZone, Timelike};

use crate::lcg_next;
use crate::read_gen::generate_read_for_chip;
use crate::scenario::ReaderScenarioConfig;

// ---------------------------------------------------------------------------
// Read mode helper
// ---------------------------------------------------------------------------

/// Parse a read mode string into `ReadMode`.
///
/// The `ReadMode` enum has serde deserialization but no standalone `FromStr`
/// implementation. We provide a lightweight local helper to avoid pulling in
/// a serde deserializer for a single string match.
fn read_mode_from_str(s: &str) -> Option<ReadMode> {
    match s {
        "raw" => Some(ReadMode::Raw),
        "event" => Some(ReadMode::Event),
        "fsls" => Some(ReadMode::FirstLastSeen),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Storage state
// ---------------------------------------------------------------------------

/// Storage state byte for extended status responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageState {
    Empty,
    HasData,
}

impl StorageState {
    pub fn wire_byte(self) -> u8 {
        match self {
            StorageState::Empty => 0x01,
            StorageState::HasData => 0x0c,
        }
    }
}

// ---------------------------------------------------------------------------
// Emulated reader state
// ---------------------------------------------------------------------------

/// Mutable state for a single emulated reader during a control session.
///
/// Constructed from `ReaderScenarioConfig` with deterministic defaults.
/// Fields mirror the values that a real IPICO reader exposes through its
/// control protocol (CONFIG3, statistics, extended status, etc.).
///
/// Invariant-bearing fields (`stored_reads`/`storage_state`,
/// `recording`/`downloading`) are private and must be mutated through
/// dedicated methods that maintain coherence.
pub struct EmulatedReaderState {
    reader_ip: String,
    fw_version: u8,
    hw_code: u8,
    hw_identifier: u16,
    banner: String,
    read_mode: ReadMode,
    config3_timeout: u8,
    tto_enabled: bool,
    recording: bool,
    clock_offset_ms: i64,
    stored_reads: u32,
    downloading: bool,
    storage_state: StorageState,
    download_progress: u32,
    download_chip_ids: Vec<u64>,
    download_seed: u64,
}

impl EmulatedReaderState {
    /// Build initial state from a scenario config and a deterministic seed.
    ///
    /// Precedence for read mode: `initial_read_mode` > `read_type` > Raw.
    pub fn from_config(cfg: &ReaderScenarioConfig, seed: u64) -> Self {
        let read_mode = cfg
            .initial_read_mode
            .as_deref()
            .and_then(read_mode_from_str)
            .or_else(|| read_mode_from_str(&cfg.read_type))
            .unwrap_or(ReadMode::Raw);

        let stored_reads = cfg.stored_reads.unwrap_or(0);

        Self {
            reader_ip: cfg.ip.clone(),
            fw_version: 0x42,
            hw_code: 0x05,
            hw_identifier: 0x5905,
            banner: format!("IPICO Emulator v2.0.0\r\nS/N: EMU-{}\r\n", cfg.ip),
            read_mode,
            config3_timeout: 5,
            tto_enabled: cfg.initial_tto_enabled.unwrap_or(false),
            recording: cfg.initial_recording.unwrap_or(false),
            clock_offset_ms: cfg.clock_offset_ms.unwrap_or(0),
            stored_reads,
            downloading: false,
            storage_state: if stored_reads > 0 {
                StorageState::HasData
            } else {
                StorageState::Empty
            },
            download_progress: 0,
            download_chip_ids: Vec::new(),
            download_seed: seed,
        }
    }

    // -- Getters --

    pub fn reader_ip(&self) -> &str {
        &self.reader_ip
    }
    pub fn fw_version(&self) -> u8 {
        self.fw_version
    }
    pub fn hw_code(&self) -> u8 {
        self.hw_code
    }
    pub fn hw_identifier(&self) -> u16 {
        self.hw_identifier
    }
    pub fn banner(&self) -> &str {
        &self.banner
    }
    pub fn read_mode(&self) -> ReadMode {
        self.read_mode
    }
    pub fn config3_timeout(&self) -> u8 {
        self.config3_timeout
    }
    pub fn tto_enabled(&self) -> bool {
        self.tto_enabled
    }
    pub fn recording(&self) -> bool {
        self.recording
    }
    pub fn clock_offset_ms(&self) -> i64 {
        self.clock_offset_ms
    }
    pub fn stored_reads(&self) -> u32 {
        self.stored_reads
    }
    pub fn downloading(&self) -> bool {
        self.downloading
    }
    pub fn storage_state(&self) -> StorageState {
        self.storage_state
    }
    pub fn download_progress(&self) -> u32 {
        self.download_progress
    }

    // -- Mutation methods (invariant-enforcing) --

    /// Set recording state. Clears downloading when turning recording on,
    /// since a real reader cannot record and download simultaneously.
    pub fn set_recording(&mut self, on: bool) {
        self.recording = on;
        if on {
            self.downloading = false;
        }
    }

    /// Set downloading state. Clears recording when turning downloading on,
    /// since a real reader cannot record and download simultaneously.
    pub fn set_downloading(&mut self, on: bool) {
        self.downloading = on;
        if on {
            self.recording = false;
        }
    }

    /// Set the stored read count, updating storage_state to match.
    pub fn set_stored_reads(&mut self, count: u32) {
        self.stored_reads = count;
        self.storage_state = if count > 0 {
            StorageState::HasData
        } else {
            StorageState::Empty
        };
    }

    /// Decrement stored reads by `count`, updating storage_state when empty.
    pub fn decrement_stored_reads(&mut self, count: u32) {
        self.stored_reads = self.stored_reads.saturating_sub(count);
        if self.stored_reads == 0 {
            self.storage_state = StorageState::Empty;
        }
    }

    /// Reset storage after erase: zero stored reads and download progress.
    pub fn reset_storage(&mut self) {
        self.stored_reads = 0;
        self.download_progress = 0;
        self.storage_state = StorageState::Empty;
    }

    pub fn set_read_mode(&mut self, mode: ReadMode) {
        self.read_mode = mode;
    }

    pub fn set_config3_timeout(&mut self, timeout: u8) {
        self.config3_timeout = timeout;
    }

    pub fn set_tto_enabled(&mut self, enabled: bool) {
        self.tto_enabled = enabled;
    }

    pub fn set_clock_offset_ms(&mut self, offset: i64) {
        self.clock_offset_ms = offset;
    }

    pub fn set_download_progress(&mut self, progress: u32) {
        self.download_progress = progress;
    }

    pub fn set_download_chip_ids(&mut self, ids: Vec<u64>) {
        self.download_chip_ids = ids;
    }
}

// ---------------------------------------------------------------------------
// Download read generation
// ---------------------------------------------------------------------------

/// Generate "stored reads" for download simulation.
///
/// Produces up to `min(max_count, state.stored_reads)` chip read strings,
/// using the same LCG as scenario event generation. The RNG is seeded with
/// `state.download_seed + sum_of_ip_bytes` and advanced past already-downloaded
/// reads (`state.download_progress` iterations) before generating new ones.
///
/// Each read uses `generate_read_for_chip` with `ReadType::RAW` and a past
/// timestamp offset by position from current time. Returns empty vec if
/// `stored_reads == 0` or `downloading == false`.
pub fn generate_download_reads(state: &mut EmulatedReaderState, max_count: u32) -> Vec<String> {
    if !state.downloading || state.stored_reads == 0 {
        return vec![];
    }

    let count = max_count.min(state.stored_reads);

    // Seed RNG the same way as generate_reader_events in scenario.rs
    let ip_byte_sum: u64 = state.reader_ip.bytes().map(|b| b as u64).sum();
    let mut rng_state: u64 = state.download_seed.wrapping_add(ip_byte_sum);

    // Advance RNG past already-downloaded reads
    for _ in 0..state.download_progress {
        rng_state = lcg_next(rng_state);
    }

    let chip_ids = if state.download_chip_ids.is_empty() {
        vec![1000u64] // fallback
    } else {
        state.download_chip_ids.clone()
    };

    let now = Local::now();
    let mut reads = Vec::with_capacity(count as usize);

    for i in 0..count {
        rng_state = lcg_next(rng_state);
        let chip_idx = (rng_state % chip_ids.len() as u64) as usize;
        let chip_id = chip_ids[chip_idx];

        // Past timestamp: offset backwards from current time by position
        let offset_secs = (count - i) as i64;
        let ts = now - chrono::Duration::seconds(offset_secs);

        let year = (ts.year() % 100) as u8;
        let month = ts.month() as u8;
        let day = ts.day() as u8;
        let hour = ts.hour() as u8;
        let minute = ts.minute() as u8;
        let second = ts.second() as u8;
        let centiseconds = (ts.nanosecond() / 10_000_000) as u8;

        let raw = generate_read_for_chip(
            chip_id,
            ReadType::RAW,
            year,
            month,
            day,
            hour,
            minute,
            second,
            centiseconds,
        );

        reads.push(format!("{}\r\n", raw));

        state.download_progress += 1;
    }

    // Decrement stored reads in one shot — this also flips storage_state
    // to Empty when reaching zero, fixing the drift bug.
    state.decrement_stored_reads(count);

    reads
}

// ---------------------------------------------------------------------------
// Response frame builder
// ---------------------------------------------------------------------------

/// Build a valid `ab`-prefixed IPICO control response frame.
///
/// The returned string includes the `\r\n` terminator and is ready to send
/// over TCP. The frame passes `ipico_core::control::parse_response()`.
fn build_response_frame(reader_id: u8, instruction: u8, data: &[u8]) -> String {
    let length = data.len() as u8;
    let mut hex_body = format!("{:02x}{:02x}{:02x}", reader_id, length, instruction);
    for &b in data {
        hex_body.push_str(&format!("{:02x}", b));
    }
    let checksum = lrc(hex_body.as_bytes());
    format!("ab{}{:02x}\r\n", hex_body, checksum)
}

// ---------------------------------------------------------------------------
// Control frame dispatcher
// ---------------------------------------------------------------------------

/// Parse an incoming `ab`-prefixed control frame and dispatch to the
/// appropriate handler. Returns zero or more response strings (each
/// terminated with `\r\n`).
///
/// Malformed frames and unknown instructions are rejected with a warning log.
pub fn handle_control_frame(state: &mut EmulatedReaderState, frame: &str) -> Vec<String> {
    // Minimum valid frame: ab + RR + LL + II + CC = 10 hex chars
    if frame.len() < 10 || !frame.starts_with("ab") {
        eprintln!(
            "[emulator] rejecting frame: too short or missing 'ab' prefix ({} chars): {:?}",
            frame.len(),
            &frame[..frame.len().min(20)]
        );
        return vec![];
    }

    let reader_id = match u8::from_str_radix(&frame[2..4], 16) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[emulator] rejecting frame: invalid reader_id hex: {:?}",
                &frame[2..4]
            );
            return vec![];
        }
    };
    let length_byte = match u8::from_str_radix(&frame[4..6], 16) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[emulator] rejecting frame: invalid length hex: {:?}",
                &frame[4..6]
            );
            return vec![];
        }
    };
    let instruction = match u8::from_str_radix(&frame[6..8], 16) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[emulator] rejecting frame: invalid instruction hex: {:?}",
                &frame[6..8]
            );
            return vec![];
        }
    };

    // Parse data bytes (between instruction and checksum).
    // Reject the entire frame if any data byte has invalid hex.
    let data_hex = &frame[8..frame.len().saturating_sub(2)]; // strip trailing checksum
    let mut data = Vec::new();
    let mut i = 0;
    while i + 1 < data_hex.len() {
        match u8::from_str_radix(&data_hex[i..i + 2], 16) {
            Ok(b) => data.push(b),
            Err(_) => {
                eprintln!(
                    "[emulator] rejecting frame: invalid hex at data offset {}: {:?}",
                    i,
                    &data_hex[i..i + 2]
                );
                return vec![];
            }
        }
        i += 2;
    }

    let is_query = length_byte == 0xff;

    match instruction {
        INSTR_GET_DATE_TIME => handle_get_date_time(state, reader_id),
        INSTR_SET_DATE_TIME => handle_set_date_time(state, reader_id, &data),
        INSTR_GET_STATISTICS => handle_get_statistics(state, reader_id),
        INSTR_CONFIG3 if is_query => handle_get_config3(state, reader_id),
        INSTR_CONFIG3 => handle_set_config3(state, reader_id, &data),
        INSTR_TAG_MESSAGE_FORMAT if is_query => handle_get_tag_message_format(state, reader_id),
        INSTR_TAG_MESSAGE_FORMAT => handle_set_tag_message_format(state, reader_id, &data),
        INSTR_PRINT_BANNER => handle_print_banner(state, reader_id),
        INSTR_EXT_STATUS if is_query => handle_get_extended_status(state, reader_id),
        INSTR_EXT_STATUS => handle_ext_status_write(state, reader_id, &data),
        INSTR_UNKNOWN_E0 => vec![build_response_frame(reader_id, INSTR_UNKNOWN_E0, &[])],
        _ => {
            eprintln!(
                "[emulator] ignoring unknown instruction: 0x{:02x}",
                instruction
            );
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// Query handlers
// ---------------------------------------------------------------------------

fn handle_get_date_time(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let now = Local::now() + chrono::Duration::milliseconds(state.clock_offset_ms);
    let year = (now.year() % 100) as u8;
    let month = now.month() as u8;
    let day = now.day() as u8;
    let dow = now.weekday().num_days_from_monday() as u8; // chrono: Mon=0..Sun=6
    // IPICO expects Mon=1, Tue=2, ..., Sat=6, Sun=0
    let day_of_week = match dow {
        6 => 0u8, // Sunday
        d => d + 1,
    };
    let hour = now.hour() as u8;
    let minute = now.minute() as u8;
    let second = now.second() as u8;
    let centisecond = (now.nanosecond() / 10_000_000) as u8;

    let data = [
        to_bcd(year).expect("year % 100 is always in BCD range"),
        to_bcd(month).expect("month 1-12 is always in BCD range"),
        to_bcd(day).expect("day 1-31 is always in BCD range"),
        day_of_week,
        to_bcd(hour).expect("hour 0-23 is always in BCD range"),
        to_bcd(minute).expect("minute 0-59 is always in BCD range"),
        to_bcd(second).expect("second 0-59 is always in BCD range"),
        centisecond,
        0x27,
    ];
    vec![build_response_frame(reader_id, INSTR_GET_DATE_TIME, &data)]
}

fn handle_get_statistics(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let data = [
        state.fw_version,
        0x00,
        0x00,
        0x00,
        0x01,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        state.read_mode.config3_value(),
        state.hw_code,
        0x00,
    ];
    vec![build_response_frame(reader_id, INSTR_GET_STATISTICS, &data)]
}

fn build_extended_status_data(state: &EmulatedReaderState) -> [u8; 13] {
    let recording_state = if state.downloading {
        0x03
    } else if state.recording {
        0x01
    } else {
        0x00
    };
    let stored_extent = state.stored_reads.saturating_mul(32);
    let dl_progress = state.download_progress.saturating_mul(32);
    let hw_be = state.hw_identifier.to_be_bytes();
    [
        recording_state,
        ((stored_extent >> 16) & 0xff) as u8,
        ((stored_extent >> 8) & 0xff) as u8,
        (stored_extent & 0xff) as u8,
        ((dl_progress >> 16) & 0xff) as u8,
        ((dl_progress >> 8) & 0xff) as u8,
        (dl_progress & 0xff) as u8,
        0x00,
        hw_be[0],
        hw_be[1],
        0x8f,
        state.storage_state.wire_byte(),
        0x00,
    ]
}

fn handle_get_extended_status(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let data = build_extended_status_data(state);
    vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &data)]
}

fn handle_get_config3(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let data = [state.read_mode.config3_value(), state.config3_timeout];
    vec![build_response_frame(reader_id, INSTR_CONFIG3, &data)]
}

fn handle_get_tag_message_format(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let field_mask = if state.tto_enabled { 0x80 } else { 0x00 };
    let data = [field_mask, 0x3f, 0x00, 0x00, 0xaa, 0x00, 0x0d, 0x0a];
    vec![build_response_frame(
        reader_id,
        INSTR_TAG_MESSAGE_FORMAT,
        &data,
    )]
}

fn handle_print_banner(state: &EmulatedReaderState, reader_id: u8) -> Vec<String> {
    let mut responses: Vec<String> = state
        .banner
        .lines()
        .map(|line| format!("{}\r\n", line))
        .collect();
    responses.push(build_response_frame(reader_id, INSTR_PRINT_BANNER, &[]));
    responses
}

// ---------------------------------------------------------------------------
// Write handlers
// ---------------------------------------------------------------------------

fn handle_set_date_time(
    state: &mut EmulatedReaderState,
    reader_id: u8,
    data: &[u8],
) -> Vec<String> {
    if data.len() < 7 {
        eprintln!(
            "[emulator] SetDateTime: insufficient data ({} bytes, need 7)",
            data.len()
        );
        return vec![];
    }
    let year = from_bcd(data[0]).unwrap_or(0) as i32;
    let month = from_bcd(data[1]).unwrap_or(1) as u32;
    let day = from_bcd(data[2]).unwrap_or(1) as u32;
    // data[3] is day_of_week — skip
    let hour = from_bcd(data[4]).unwrap_or(0) as u32;
    let minute = from_bcd(data[5]).unwrap_or(0) as u32;
    let second = from_bcd(data[6]).unwrap_or(0) as u32;

    let full_year = 2000 + year;
    match Local
        .with_ymd_and_hms(full_year, month, day, hour, minute, second)
        .single()
    {
        Some(target) => {
            let now = Local::now();
            let diff = target - now;
            state.set_clock_offset_ms(diff.num_milliseconds());
        }
        None => {
            eprintln!(
                "[emulator] SetDateTime: invalid date {full_year}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
            );
        }
    }
    vec![build_response_frame(reader_id, INSTR_SET_DATE_TIME, &[])]
}

fn handle_set_config3(state: &mut EmulatedReaderState, reader_id: u8, data: &[u8]) -> Vec<String> {
    if data.len() < 2 {
        eprintln!(
            "[emulator] SetConfig3: insufficient data ({} bytes)",
            data.len()
        );
        return vec![];
    }
    if let Some(mode) = ReadMode::from_config3(data[0]) {
        state.set_read_mode(mode);
    }
    state.set_config3_timeout(data[1]);
    vec![build_response_frame(reader_id, INSTR_CONFIG3, &[])]
}

fn handle_set_tag_message_format(
    state: &mut EmulatedReaderState,
    reader_id: u8,
    data: &[u8],
) -> Vec<String> {
    if data.is_empty() {
        eprintln!("[emulator] SetTagMessageFormat: no data bytes");
        return vec![];
    }
    state.set_tto_enabled(data[0] & 0x80 != 0);
    vec![build_response_frame(
        reader_id,
        INSTR_TAG_MESSAGE_FORMAT,
        &[],
    )]
}

fn handle_ext_status_write(
    state: &mut EmulatedReaderState,
    reader_id: u8,
    data: &[u8],
) -> Vec<String> {
    if data.is_empty() {
        eprintln!("[emulator] ExtStatusWrite: no data bytes");
        return vec![];
    }
    match data[0] {
        0x00 => {
            // SetRecordingState
            if data.len() < 2 {
                eprintln!(
                    "[emulator] SetRecordingState: insufficient data ({} bytes)",
                    data.len()
                );
                return vec![];
            }
            state.set_recording(data[1] != 0);
            vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &[])]
        }
        0x01 => {
            // SetAccessMode
            if data.len() < 2 {
                eprintln!(
                    "[emulator] SetAccessMode: insufficient data ({} bytes)",
                    data.len()
                );
                return vec![];
            }
            state.set_downloading(data[1] != 0);
            if state.downloading {
                let ext_data = build_extended_status_data(state);
                vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &ext_data)]
            } else {
                vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &[])]
            }
        }
        0x02 => {
            // InitDownload
            state.set_download_progress(0);
            vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &[])]
        }
        0x07 => {
            // ConfigureDownload / CleanupDownload — no-op
            vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &[])]
        }
        0xd0 => {
            // TriggerErase
            state.reset_storage();
            vec![build_response_frame(reader_id, INSTR_EXT_STATUS, &[])]
        }
        _ => {
            eprintln!(
                "[emulator] ExtStatusWrite: unknown sub-command 0x{:02x}",
                data[0]
            );
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ReaderScenarioConfig;

    /// Helper to build a minimal `ReaderScenarioConfig` with defaults.
    fn base_config() -> ReaderScenarioConfig {
        ReaderScenarioConfig {
            ip: "192.168.1.100".to_string(),
            port: 10000,
            read_type: "raw".to_string(),
            chip_ids: vec![1000],
            events_per_second: 10,
            total_events: 100,
            start_delay_ms: 0,
            faults: vec![],
            initial_read_mode: None,
            initial_tto_enabled: None,
            initial_recording: None,
            stored_reads: None,
            clock_offset_ms: None,
        }
    }

    #[test]
    fn state_from_config_defaults() {
        let cfg = base_config();
        let state = EmulatedReaderState::from_config(&cfg, 42);

        assert_eq!(state.read_mode(), ReadMode::Raw);
        assert!(!state.tto_enabled());
        assert!(!state.recording());
        assert_eq!(state.stored_reads(), 0);
        assert_eq!(state.clock_offset_ms(), 0);
        assert!(state.banner().contains("EMU"));
        assert_eq!(state.storage_state(), StorageState::Empty);
        assert!(!state.downloading());
        assert_eq!(state.download_progress(), 0);
        assert_eq!(state.fw_version(), 0x42);
        assert_eq!(state.hw_code(), 0x05);
        assert_eq!(state.hw_identifier(), 0x5905);
        assert_eq!(state.config3_timeout(), 5);
    }

    // -- build_response_frame tests --

    #[test]
    fn build_ack_frame_for_set_datetime() {
        let frame = build_response_frame(0x00, 0x01, &[]);
        let parsed = ipico_core::control::parse_response(frame.trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), 0x01);
        assert!(parsed.data().is_empty());
    }

    #[test]
    fn build_data_frame_for_config3() {
        let frame = build_response_frame(0x00, 0x09, &[0x00, 0x05]);
        let parsed = ipico_core::control::parse_response(frame.trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), 0x09);
        assert_eq!(parsed.data(), &[0x00, 0x05]);
    }

    #[test]
    fn build_data_frame_for_extended_status_13_bytes() {
        let data = [
            0x01, 0x00, 0x0b, 0x2f, 0x00, 0x00, 0x00, 0x00, 0x59, 0x05, 0x8f, 0x0c, 0x00,
        ];
        let frame = build_response_frame(0x00, 0x4b, &data);
        let parsed = ipico_core::control::parse_response(frame.trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), 0x4b);
        assert_eq!(parsed.data().len(), 13);
    }

    #[test]
    fn state_from_config_with_overrides() {
        let mut cfg = base_config();
        cfg.initial_read_mode = Some("fsls".to_string());
        cfg.initial_tto_enabled = Some(true);
        cfg.initial_recording = Some(true);
        cfg.stored_reads = Some(500);
        cfg.clock_offset_ms = Some(-1500);

        let state = EmulatedReaderState::from_config(&cfg, 99);

        assert_eq!(state.read_mode(), ReadMode::FirstLastSeen);
        assert!(state.tto_enabled());
        assert!(state.recording());
        assert_eq!(state.stored_reads(), 500);
        assert_eq!(state.clock_offset_ms(), -1500);
        assert_eq!(state.storage_state(), StorageState::HasData);
        assert_eq!(state.download_seed, 99);
    }

    // -- handle_control_frame tests --

    use ipico_core::control::{
        self, Command, RecordingState, decode_config3, decode_date_time, decode_extended_status,
        decode_statistics, decode_tag_message_format, encode_command, parse_response,
    };

    fn make_test_state() -> EmulatedReaderState {
        let cfg = base_config();
        EmulatedReaderState::from_config(&cfg, 42)
    }

    /// Convert a Command into the trimmed frame string suitable for handle_control_frame.
    fn cmd_to_frame(cmd: &Command) -> String {
        let bytes = encode_command(cmd, 0x00).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        s.trim_end().to_string()
    }

    // -- Query tests --

    #[test]
    fn handle_get_date_time_returns_valid_frame() {
        let mut state = make_test_state();
        let frame = cmd_to_frame(&Command::GetDateTime);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), INSTR_GET_DATE_TIME);
        let dt = decode_date_time(&parsed).unwrap();
        // Just verify it decodes successfully with reasonable values
        assert!(dt.year <= 99);
        assert!((1..=12).contains(&dt.month));
        assert!((1..=31).contains(&dt.day));
    }

    #[test]
    fn handle_get_statistics_returns_14_byte_response() {
        let mut state = make_test_state();
        let frame = cmd_to_frame(&Command::GetStatistics);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        let stats = decode_statistics(&parsed).unwrap();
        assert_eq!(stats.fw_version_string(), "4.2");
        assert_eq!(stats.hw_code, 0x05);
    }

    #[test]
    fn handle_get_extended_status_returns_13_byte_response() {
        let mut state = make_test_state();
        state.set_recording(true);
        state.set_stored_reads(500);
        let frame = cmd_to_frame(&Command::GetExtendedStatus);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        let ext = decode_extended_status(&parsed).unwrap();
        assert_eq!(ext.recording_state, RecordingState::On);
        assert_eq!(ext.stored_data_extent, 500 * 32);
        assert_eq!(ext.hw_identifier, 0x5905);
        assert_eq!(ext.hw_config, 0x8f);
        assert_eq!(ext.storage_state, StorageState::HasData.wire_byte());
    }

    #[test]
    fn handle_get_config3_returns_current_mode() {
        let mut state = make_test_state();
        state.set_read_mode(ReadMode::Event);
        state.set_config3_timeout(8);
        let frame = cmd_to_frame(&Command::GetConfig3);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        let (mode, timeout) = decode_config3(&parsed).unwrap();
        assert_eq!(mode, ReadMode::Event);
        assert_eq!(timeout, 8);
    }

    #[test]
    fn handle_get_tag_message_format_reflects_tto() {
        let mut state = make_test_state();
        state.set_tto_enabled(true);
        let frame = cmd_to_frame(&Command::GetTagMessageFormat);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        let fmt = decode_tag_message_format(&parsed).unwrap();
        assert!(fmt.tto_enabled());
        assert_eq!(fmt.field_mask & 0x80, 0x80);
    }

    #[test]
    fn handle_print_banner_returns_lines_plus_ack() {
        let mut state = make_test_state();
        let frame = cmd_to_frame(&Command::PrintBanner);
        let responses = handle_control_frame(&mut state, &frame);
        assert!(responses.len() >= 2, "expected banner lines + ACK");
        // Last response should be an ACK frame
        let last = responses.last().unwrap();
        assert!(
            last.starts_with("ab"),
            "last response should be ab-prefixed ACK"
        );
        let parsed = parse_response(last.trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), INSTR_PRINT_BANNER);
        // First responses should NOT be ab-prefixed
        for line in &responses[..responses.len() - 1] {
            assert!(
                !line.starts_with("ab"),
                "banner line should not be ab-prefixed: {}",
                line
            );
        }
    }

    #[test]
    fn handle_init_e0_returns_ack() {
        let mut state = make_test_state();
        let frame = cmd_to_frame(&Command::InitE0);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), INSTR_UNKNOWN_E0);
        assert!(parsed.data().is_empty());
    }

    // -- Write tests --

    #[test]
    fn handle_set_config3_updates_mode() {
        let mut state = make_test_state();
        assert_eq!(state.read_mode(), ReadMode::Raw);
        let frame = cmd_to_frame(&Command::SetConfig3 {
            mode: ReadMode::Event,
            timeout: 8,
        });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        assert_eq!(parsed.instruction(), INSTR_CONFIG3);
        assert_eq!(state.read_mode(), ReadMode::Event);
        assert_eq!(state.config3_timeout(), 8);
    }

    #[test]
    fn handle_set_tag_message_format_updates_tto() {
        let mut state = make_test_state();
        assert!(!state.tto_enabled());
        let fmt = control::TagMessageFormat {
            field_mask: 0x80,
            id_byte_mask: 0x3f,
            ascii_header_1: 0x00,
            ascii_header_2: 0x00,
            binary_header_1: 0xaa,
            binary_header_2: 0x00,
            trailer_1: 0x0d,
            trailer_2: 0x0a,
            separator: None,
        };
        let frame = cmd_to_frame(&Command::SetTagMessageFormat { format: fmt });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        assert!(state.tto_enabled());
    }

    #[test]
    fn handle_set_recording_on_and_off() {
        let mut state = make_test_state();
        assert!(!state.recording());

        let frame = cmd_to_frame(&Command::SetRecordingState { on: true });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        assert!(state.recording());

        let frame = cmd_to_frame(&Command::SetRecordingState { on: false });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        assert!(!state.recording());
    }

    #[test]
    fn handle_set_access_mode_on_sets_downloading() {
        let mut state = make_test_state();
        state.set_recording(true);
        state.set_stored_reads(100);

        let frame = cmd_to_frame(&Command::SetAccessMode { on: true });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        assert!(state.downloading());

        // Response should be 13-byte extended status with recording_state=Downloading
        let parsed = parse_response(responses[0].trim_end().as_bytes()).unwrap();
        let ext = decode_extended_status(&parsed).unwrap();
        assert_eq!(ext.recording_state, RecordingState::Downloading);
    }

    #[test]
    fn handle_trigger_erase_clears_stored_reads() {
        let mut state = make_test_state();
        state.set_stored_reads(500);
        state.set_download_progress(100);

        let frame = cmd_to_frame(&Command::TriggerErase);
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        assert_eq!(state.stored_reads(), 0);
        assert_eq!(state.storage_state(), StorageState::Empty);
        assert_eq!(state.download_progress(), 0);
    }

    #[test]
    fn handle_set_datetime_updates_clock_offset() {
        let mut state = make_test_state();
        let now = chrono::Local::now();
        let future = now + chrono::Duration::hours(1);
        let year = (future.year() % 100) as u8;
        let month = future.month() as u8;
        let day = future.day() as u8;
        let dow = {
            let d = future.weekday().num_days_from_monday();
            if d == 6 { 0u8 } else { (d + 1) as u8 }
        };
        let hour = future.hour() as u8;
        let minute = future.minute() as u8;
        let second = future.second() as u8;

        let frame = cmd_to_frame(&Command::SetDateTime {
            year,
            month,
            day,
            day_of_week: dow,
            hour,
            minute,
            second,
        });
        let responses = handle_control_frame(&mut state, &frame);
        assert_eq!(responses.len(), 1);
        // clock_offset should be approximately 3600000ms (1 hour), within 2s tolerance
        let diff = (state.clock_offset_ms() - 3_600_000).unsigned_abs();
        assert!(
            diff < 2000,
            "expected clock_offset_ms ~3600000, got {}",
            state.clock_offset_ms()
        );
    }

    // -- Download read generation tests --

    #[test]
    fn generate_download_reads_produces_valid_chip_reads() {
        let mut state = make_test_state();
        state.set_stored_reads(3);
        state.set_download_chip_ids(vec![1234, 5678]);
        state.download_seed = 42;
        state.set_downloading(true);

        let reads = generate_download_reads(&mut state, 3);
        assert_eq!(reads.len(), 3);
        assert_eq!(state.stored_reads(), 0);
        assert_eq!(state.download_progress(), 3);

        for read in &reads {
            let trimmed = read.trim();
            assert!(trimmed.starts_with("aa"));
            assert!(ipico_core::read::ChipRead::try_from(trimmed).is_ok());
        }
    }

    #[test]
    fn generate_download_reads_stops_at_stored_reads() {
        let mut state = make_test_state();
        state.set_stored_reads(2);
        state.set_downloading(true);

        let reads = generate_download_reads(&mut state, 10);
        assert_eq!(reads.len(), 2);
        assert_eq!(state.stored_reads(), 0);
    }

    #[test]
    fn generate_download_reads_returns_empty_when_not_downloading() {
        let mut state = make_test_state();
        state.set_stored_reads(100);
        // downloading is false by default from from_config

        let reads = generate_download_reads(&mut state, 10);
        assert!(reads.is_empty());
    }

    // -- Invariant enforcement tests --

    #[test]
    fn storage_state_flips_to_empty_after_full_download() {
        let mut state = make_test_state();
        state.set_stored_reads(3);
        state.set_downloading(true);
        assert_eq!(state.storage_state(), StorageState::HasData);

        let reads = generate_download_reads(&mut state, 3);
        assert_eq!(reads.len(), 3);
        assert_eq!(state.stored_reads(), 0);
        assert_eq!(state.storage_state(), StorageState::Empty);
    }

    #[test]
    fn set_downloading_clears_recording() {
        let mut state = make_test_state();
        state.set_recording(true);
        assert!(state.recording());

        state.set_downloading(true);
        assert!(state.downloading());
        assert!(!state.recording());
    }

    #[test]
    fn set_recording_clears_downloading() {
        let mut state = make_test_state();
        state.set_downloading(true);
        assert!(state.downloading());

        state.set_recording(true);
        assert!(state.recording());
        assert!(!state.downloading());
    }

    // -- Malformed frame tests --

    #[test]
    fn handle_control_frame_rejects_empty_frame() {
        let mut state = make_test_state();
        assert!(handle_control_frame(&mut state, "").is_empty());
    }

    #[test]
    fn handle_control_frame_rejects_short_frame() {
        let mut state = make_test_state();
        assert!(handle_control_frame(&mut state, "ab01").is_empty());
    }

    #[test]
    fn handle_control_frame_rejects_non_ab_prefix() {
        let mut state = make_test_state();
        assert!(handle_control_frame(&mut state, "cd00ff0a00").is_empty());
    }

    #[test]
    fn handle_control_frame_rejects_invalid_hex_in_data() {
        let mut state = make_test_state();
        // Valid header (reader=00, len=02, instr=09) but "ZZ" in data
        assert!(handle_control_frame(&mut state, "ab00020900ZZ00").is_empty());
    }

    #[test]
    fn handle_control_frame_ignores_unknown_instruction() {
        let mut state = make_test_state();
        // Valid frame structure with unknown instruction 0xAA
        assert!(handle_control_frame(&mut state, "ab00ffaa00").is_empty());
    }

    #[test]
    fn handle_set_config3_rejects_short_data() {
        let mut state = make_test_state();
        let original_mode = state.read_mode();
        // Build a frame with instruction=CONFIG3 but only 1 data byte
        let frame = build_response_frame(0x00, INSTR_CONFIG3, &[0x01]);
        let trimmed = frame.trim_end();
        let responses = handle_control_frame(&mut state, trimmed);
        assert!(responses.is_empty());
        assert_eq!(
            state.read_mode(),
            original_mode,
            "state should not be mutated"
        );
    }
}
