//! Standard I/O transport executable and library runner for Shucked LSP.

use anyhow::Result;

/// Run the Shucked language server over standard input and output.
pub fn run() -> Result<()> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    shucked_lsp::run_connection(connection)?;
    match io_threads.join() {
        Ok(()) => Ok(()),
        Err(io) => Err(anyhow::Error::new(io).context("IO thread error")),
    }
}
