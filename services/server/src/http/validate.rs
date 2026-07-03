//! Boundary limits for device-supplied strings. Everything here bounds what a
//! voucher holder or pending device can persist and have re-served on /status.

pub(crate) const MAX_ID_LEN: usize = 256; // endpoint_id, stream_id, chip_id, token_id path param
pub(crate) const MAX_NAME_LEN: usize = 256; // display_name, division
pub(crate) const MAX_DIRECT_ADDRS: usize = 32;
pub(crate) const MAX_ADDR_LEN: usize = 256;
pub(crate) const MAX_CATALOG_STREAMS: usize = 256;
pub(crate) const MAX_VOUCHER_LEN: usize = 128;

/// `Err(field name)` when `value` exceeds `max` bytes.
pub(crate) fn check_len(field: &'static str, value: &str, max: usize) -> Result<(), &'static str> {
    if value.len() > max {
        Err(field)
    } else {
        Ok(())
    }
}
