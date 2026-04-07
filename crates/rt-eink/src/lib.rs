// Display coordinates are always small (250x122), so u32→i32 casts are safe.
#![allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]

#[cfg(feature = "hardware")]
pub mod driver;
pub mod layout;
pub mod render;
pub mod state;
pub mod task;
