//! Calling a tool of a running window, for scripts and tests.

use std::path::Path;

use base64::Engine;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientConfig};

/// One piece of a tool's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Text(String),
    /// An image, decoded to its file bytes.
    Image {
        mime_type: String,
        data: Vec<u8>,
    },
}

struct Client;

impl rmcp::ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

/// Connects to the window listening on `socket`, calls `tool` with
/// `arguments` and returns what it answered. A tool that reports failure
/// gives an `Err` with its message.
pub fn call(
    socket: &Path,
    tool: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<Output>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|err| format!("cannot connect to {}: {err}", socket.display()))?;
        let client = Client.serve(stream).await.map_err(|err| err.to_string())?;
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let result = client
            .call_tool(params)
            .await
            .map_err(|err| err.to_string())?;
        let _ = client.cancel().await;

        let mut outputs = Vec::new();
        for block in &result.content {
            if let Some(text) = block.as_text() {
                outputs.push(Output::Text(text.text.clone()));
            } else if let Some(image) = block.as_image() {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .map_err(|err| format!("the tool sent a broken image: {err}"))?;
                outputs.push(Output::Image {
                    mime_type: image.mime_type.clone(),
                    data,
                });
            }
        }
        if result.is_error == Some(true) {
            let message = outputs.iter().find_map(|output| match output {
                Output::Text(text) => Some(text.clone()),
                Output::Image { .. } => None,
            });
            return Err(message.unwrap_or_else(|| format!("{tool} failed")));
        }
        Ok(outputs)
    })
}
