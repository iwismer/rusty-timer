// Display coordinates are always small, so u32→i32 casts are safe.
#![allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]

pub mod eink;
pub mod lcd;
pub mod state;
pub mod task;
