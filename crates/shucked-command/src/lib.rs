//! Shared command resolution with explicit environment evidence.
//!
//! Resolution, validation, and inventory comparison are pure. Only [`host`]
//! inspects the filesystem, and it never executes a candidate command.

mod context;
pub mod host;
mod inventory;
mod manifests;
mod provider;
mod resolver;
mod validation;

pub use context::*;
pub use inventory::*;
pub use manifests::*;
pub use provider::*;
pub use resolver::*;
pub use validation::*;
