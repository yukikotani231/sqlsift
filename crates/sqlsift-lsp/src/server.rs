use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics::to_lsp_diagnostics;
use crate::state::ServerState;

pub struct Backend {
    client: Client,
    state: Arc<RwLock<ServerState>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(ServerState::new())),
        }
    }

    /// Analyze a document and publish diagnostics
    async fn publish_diagnostics_for(&self, uri: Url, text: &str) {
        let state = self.state.read().await;
        let diagnostics = state.analyze_document(text);
        let lsp_diagnostics = to_lsp_diagnostics(&diagnostics, &state.disabled_rules);
        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    /// Re-analyze all open documents and publish diagnostics
    async fn reanalyze_all_open_documents(&self) {
        let uris_and_texts: Vec<(Url, String)> = {
            let state = self.state.read().await;
            state
                .open_documents
                .iter()
                .map(|(uri, text)| (uri.clone(), text.clone()))
                .collect()
        };

        for (uri, text) in uris_and_texts {
            self.publish_diagnostics_for(uri, &text).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace root for config loading
        if let Some(root_uri) = params.root_uri {
            if let Ok(path) = root_uri.to_file_path() {
                let mut state = self.state.write().await;
                state.load_config(&path);
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Build catalog from schema files
        let errors = {
            let mut state = self.state.write().await;
            state.rebuild_catalog()
        };

        let schema_count = self.state.read().await.schema_files.len();
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "sqlsift LSP initialized ({} schema file(s) loaded)",
                    schema_count
                ),
            )
            .await;

        for error in errors {
            self.client.log_message(MessageType::WARNING, error).await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();

        {
            let mut state = self.state.write().await;
            state.open_documents.insert(uri.clone(), text.clone());
        }

        self.publish_diagnostics_for(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        // FULL sync: first content change contains the entire document
        if let Some(change) = params.content_changes.into_iter().next() {
            let text = change.text;

            {
                let mut state = self.state.write().await;
                state.open_documents.insert(uri.clone(), text.clone());
            }

            self.publish_diagnostics_for(uri, &text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        // Check if saved file is a schema file
        let is_schema = if let Ok(path) = uri.to_file_path() {
            let state = self.state.read().await;
            state.is_schema_file(&path)
        } else {
            false
        };

        if is_schema {
            // Rebuild catalog and re-analyze all open documents
            let errors = {
                let mut state = self.state.write().await;
                state.rebuild_catalog()
            };

            for error in errors {
                self.client.log_message(MessageType::WARNING, error).await;
            }

            self.client
                .log_message(MessageType::INFO, "Schema updated, re-analyzing documents")
                .await;

            self.reanalyze_all_open_documents().await;
        } else if let Some(text) = params.text {
            // Re-analyze the saved document
            self.publish_diagnostics_for(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        {
            let mut state = self.state.write().await;
            state.open_documents.remove(&uri);
        }

        // Clear diagnostics for closed document
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let state = self.state.read().await;
        let text = match state.open_documents.get(uri) {
            Some(t) => t,
            None => return Ok(None),
        };

        let word = match word_at_position(text, position.line as usize, position.character as usize)
        {
            Some(w) => w,
            None => return Ok(None),
        };

        match state.hover_info(&word) {
            Some(markdown) => Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: None,
            })),
            None => Ok(None),
        }
    }
}

/// Extract the SQL identifier at the given line/character position
fn word_at_position(text: &str, line: usize, character: usize) -> Option<String> {
    let target_line = text.lines().nth(line)?;
    let bytes = target_line.as_bytes();

    if character >= bytes.len() || !is_ident_char(bytes[character]) {
        return None;
    }

    let mut start = character;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = character;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    Some(target_line[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_at_position_middle() {
        let text = "SELECT name FROM users";
        assert_eq!(word_at_position(text, 0, 8), Some("name".to_string()));
    }

    #[test]
    fn test_word_at_position_start() {
        let text = "SELECT name FROM users";
        assert_eq!(word_at_position(text, 0, 0), Some("SELECT".to_string()));
    }

    #[test]
    fn test_word_at_position_end() {
        let text = "SELECT name FROM users";
        assert_eq!(word_at_position(text, 0, 18), Some("users".to_string()));
    }

    #[test]
    fn test_word_at_position_multiline() {
        let text = "SELECT id\nFROM users";
        assert_eq!(word_at_position(text, 1, 5), Some("users".to_string()));
    }

    #[test]
    fn test_word_at_position_on_space() {
        let text = "SELECT name FROM users";
        assert_eq!(word_at_position(text, 0, 6), None);
    }

    #[test]
    fn test_word_at_position_past_line_end() {
        let text = "SELECT";
        assert_eq!(word_at_position(text, 0, 10), None);
    }

    #[test]
    fn test_word_at_position_underscore() {
        let text = "SELECT user_name FROM users";
        assert_eq!(word_at_position(text, 0, 10), Some("user_name".to_string()));
    }
}
