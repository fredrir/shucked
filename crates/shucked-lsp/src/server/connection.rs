use lsp_server as lsp;

/// Channel sender for outgoing LSP messages to the client.
pub type ConnectionSender = crossbeam::channel::Sender<lsp::Message>;

/// Connection initializer wrapping an abstract LSP connection.
pub struct ConnectionInitializer {
    connection: lsp::Connection,
}

impl ConnectionInitializer {
    /// Create a connection initializer from a generic `lsp_server::Connection`.
    pub fn from_connection(connection: lsp::Connection) -> Self {
        Self { connection }
    }

    pub(crate) fn initialize_start(
        &self,
    ) -> crate::Result<(lsp::RequestId, lsp_types::InitializeParams)> {
        let (id, mut params) = self.connection.initialize_start()?;
        // lsp-types names this field singular; LSP clients send workspace.diagnostics.
        if let Some(workspace) = params
            .pointer_mut("/capabilities/workspace")
            .and_then(serde_json::Value::as_object_mut)
            && let Some(diagnostics) = workspace.get("diagnostics").cloned()
        {
            workspace.insert("diagnostic".into(), diagnostics);
        }
        Ok((id, serde_json::from_value(params)?))
    }

    pub(crate) fn initialize_finish(
        self,
        id: lsp::RequestId,
        server_capabilities: &lsp_types::ServerCapabilities,
        name: &str,
        version: &str,
    ) -> crate::Result<lsp_server::Connection> {
        self.connection.initialize_finish(
            id,
            serde_json::json!({
                "capabilities": server_capabilities,
                "serverInfo": {
                    "name": name,
                    "version": version
                }
            }),
        )?;
        Ok(self.connection)
    }
}
