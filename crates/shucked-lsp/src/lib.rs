#![warn(missing_docs)]
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

//! Language Server Protocol implementation for Shucked.
//!
//! Provides the language server state, session management, handlers, capabilities,
//! and transport-independent event loop. The native stdio runner is provided by `shucked-server`.

use std::num::NonZeroUsize;

pub use capabilities::server_capabilities;
pub use edit::{DocumentKey, PositionEncoding, TextDocument};
pub use handlers::generate_diagnostics;
use lsp_types::CodeActionKind;
pub use server::Server;
pub use session::{
    Client, ClientOptions, DocumentQuery, DocumentSnapshot, GlobalOptions, Session, Workspace,
    Workspaces,
};

pub mod capabilities;
pub mod edit;
pub(crate) mod editor;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
pub mod handlers;
pub(crate) mod logging;
pub mod server;
pub mod session;

pub(crate) use handlers::{
    analysis, call_hierarchy, editor_features, fix, folding, format, lint, resolve, selection,
    symbols, workspace_diagnostics, workspace_functions, workspace_variables,
};
pub(crate) use session::workspace;

/// The server name reported in initialize response.
pub const SERVER_NAME: &str = "shucked";
/// Diagnostic identifier used by the server.
pub const DIAGNOSTIC_NAME: &str = "shucked";

/// CodeActionKind for fixing all auto-fixable issues.
pub const SOURCE_FIX_ALL_SHUCKED: CodeActionKind = CodeActionKind::new("source.fixAll.shucked");
/// Legacy CodeActionKind for fixing all auto-fixable issues.
pub const SOURCE_FIX_ALL_SHUCK: CodeActionKind = CodeActionKind::new("source.fixAll.shuck");

pub(crate) type Result<T> = anyhow::Result<T>;

pub(crate) fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Run the Shucked language server over a generic LSP connection.
pub fn run_connection(connection: lsp_server::Connection) -> Result<()> {
    let four = NonZeroUsize::try_from(4usize)
        .map_err(|_| anyhow::anyhow!("failed to create non-zero worker count"))?;
    let worker_threads = std::thread::available_parallelism()
        .unwrap_or(four)
        .min(four);
    match start_server(
        worker_threads,
        server::ConnectionInitializer::from_connection(connection),
    )? {
        Some(server) => server.run(),
        None => Ok(()),
    }
}

/// Start the server with the given worker threads and connection initializer.
pub fn start_server(
    worker_threads: NonZeroUsize,
    connection: server::ConnectionInitializer,
) -> Result<Option<Server>> {
    match Server::new(worker_threads, connection) {
        Ok(server) => Ok(Some(server)),
        Err(error) if is_disconnected(&error) => Ok(None),
        Err(error) => Err(error.context("Failed to start server")),
    }
}

fn is_disconnected(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<lsp_server::ProtocolError>()
        .is_some_and(lsp_server::ProtocolError::channel_is_disconnected)
}
