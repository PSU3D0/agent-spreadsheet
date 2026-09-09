#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

pub mod analysis;
#[cfg(feature = "recalc")]
mod canonical_outcome;
#[cfg(feature = "recalc")]
pub mod canonical_lifecycle;
pub mod canonical_optional;
pub mod canonical_reads;
#[cfg(feature = "recalc")]
pub mod canonical_write;
pub mod caps;
#[cfg(all(not(target_arch = "wasm32"), feature = "recalc", feature = "cli"))]
pub mod cli;
pub mod config;
pub mod core;
#[cfg(feature = "recalc")]
pub mod diff;
pub mod errors;
pub mod execution_context;
#[cfg(feature = "recalc")]
pub mod fork;
pub mod formula;
pub mod hostfs;
pub mod model;
pub mod operations;
pub mod read;
pub mod read_context;
#[cfg(feature = "recalc")]
pub mod recalc;
/// Native raster screenshot backend. Present only with the `render` feature.
#[cfg(feature = "render")]
pub mod render;
pub mod repository;
pub mod response_prune;
pub mod rules;
pub mod runtime;
pub mod security;
pub mod session;
#[cfg(all(feature = "native-fs", feature = "recalc-formualizer", not(target_arch = "wasm32")))]
pub mod native_resident;
#[cfg(all(feature = "native-fs", feature = "recalc-formualizer", not(target_arch = "wasm32")))]
pub mod native_host;
#[cfg(all(feature = "native-fs", feature = "recalc-formualizer", not(target_arch = "wasm32")))]
mod native_export;
#[cfg(feature = "recalc")]
pub mod session_history;
pub mod resident_export;
pub mod state;
pub mod styles;
pub mod tools;
pub mod types;
pub mod utils;
pub mod verification;
pub mod workbook;
mod xlsx_import;
mod xlsx_export;
pub mod write;
