//! Planning a Compress encode (the job itself is [`crate::jobs::compress`]).

pub mod plan;

pub use plan::{plan_compress, CompressDecision, CompressError, CompressPlan, SIZE_LIMIT_BYTES};
