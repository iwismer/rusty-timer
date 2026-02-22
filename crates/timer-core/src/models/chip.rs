// Re-export core IPICO parsing types from ipico-core.
// The canonical implementation lives in crates/ipico-core/src/read.rs.
pub use ipico_core::read::{ChipRead, ReadType};

/// A struct for mapping a chip to a bib number
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Clone)]
pub struct ChipBib {
    pub id: String,
    pub bib: i32,
}
