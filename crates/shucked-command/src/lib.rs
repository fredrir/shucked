//! Shared command resolution with explicit environment evidence.
//!
//! Resolution, validation, and inventory comparison are pure. [`host`] reads the
//! filesystem; [`metadata`] executes fixed audited queries only with explicit authority.

mod context;
pub mod host;
mod inventory;
mod manifests;
pub mod metadata;
pub mod process;
mod provider;
mod resolver;
mod validation;

pub use context::*;
pub use inventory::*;
pub use manifests::*;
pub use provider::*;
pub use resolver::*;
pub use validation::*;
