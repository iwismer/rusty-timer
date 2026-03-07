//! IPICO reader TCP control protocol encoding/decoding.
//!
//! Pure functions — no async, no I/O. All frame encoding/decoding for the
//! `ab`-prefixed control protocol described in `docs/ipico-control-protocol.md`.

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
}
