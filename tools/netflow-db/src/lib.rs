//! Rust implementation of the ATLANTIS NetFlow pipeline.

#![forbid(unsafe_code)]

/// The persisted pipeline contract version produced by this implementation.
pub const PIPELINE_CONTRACT_VERSION: u32 = 3;

pub mod compare;
pub mod config;
pub mod domain;
pub mod export;
pub mod ingest;
pub mod maad;
pub(crate) mod nfdump;
pub mod normalize;
pub mod operations;
pub mod pipeline;
pub mod prepare;
pub mod provenance;
pub mod publish;
pub mod registry;
pub mod storage;
pub mod verify;
