//! Compression planning.

pub mod plan;

pub use plan::{plan_compress, CompressDecision, CompressError, CompressPlan, SIZE_LIMIT_BYTES};
