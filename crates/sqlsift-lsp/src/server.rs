use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionOptions, CompletionParams,
    CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, MarkupContent,
    MarkupKind, MessageType, NumberOrString, Position, Range, SaveOptions, ServerCapabilities,
    TextDocumentItem, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Url, WorkspaceEdit,
};
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
        let diagnostics = state.document_diagnostics(&uri, text);
        let lsp_diagnostics = to_lsp_diagnostics(&diagnostics, text);
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
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), " ".to_string()]),
                    ..Default::default()
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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

        let config_warnings = std::mem::take(&mut self.state.write().await.config_warnings);
        for warning in config_warnings {
            self.client
                .show_message(MessageType::WARNING, warning)
                .await;
        }

        let schema_count = self.state.read().await.schema_files.len();
        self.client
            .log_message(
                MessageType::INFO,
                format!("sqlsift LSP initialized ({schema_count} schema file(s) loaded)"),
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
        let TextDocumentItem {
            uri,
            text,
            language_id,
            ..
        } = params.text_document;

        {
            let mut state = self.state.write().await;
            state.open_documents.insert(uri.clone(), text.clone());
            state.document_languages.insert(uri.clone(), language_id);
        }

        self.publish_diagnostics_for(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
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
        let uri = params.text_document.uri;

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
        let uri = params.text_document.uri;

        {
            let mut state = self.state.write().await;
            state.open_documents.remove(&uri);
            state.document_languages.remove(&uri);
        }

        // Clear diagnostics for closed document
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let state = self.state.read().await;
        // Schema names are offered only in SQL documents, not in the code of
        // TypeScript / JavaScript ones
        if state.is_embedded_sql_document(uri) {
            return Ok(None);
        }
        let Some(text) = state.open_documents.get(uri) else {
            return Ok(None);
        };

        let Some(word) =
            word_at_position(text, position.line as usize, position.character as usize)
        else {
            return Ok(None);
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let state = self.state.read().await;
        if state.is_embedded_sql_document(&params.text_document_position.text_document.uri) {
            return Ok(None);
        }
        let items = state.completion_items();

        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::Array(items)))
        }
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let state = self.state.read().await;
        if state.is_embedded_sql_document(uri) {
            return Ok(None);
        }
        let Some(text) = state.open_documents.get(uri) else {
            return Ok(None);
        };

        let has_bom = text.starts_with('\u{feff}');
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut actions = Vec::new();

        for diag in &params.context.diagnostics {
            if diag.source.as_deref() != Some("sqlsift") {
                continue;
            }
            let Some(code) = (match &diag.code {
                Some(NumberOrString::String(s)) => Some(s.as_str()),
                _ => None,
            }) else {
                continue;
            };

            let line_idx = diag.range.start.line as usize;
            let line_text = text.lines().nth(line_idx).unwrap_or("");
            let clean_line = line_text.strip_prefix('\u{feff}').unwrap_or(line_text);
            let indent: String = clean_line
                .chars()
                .take_while(|c| c.is_whitespace() && *c != '\r' && *c != '\n')
                .collect();

            // Disable for this line (omitted if line starts inside a multi-line string literal)
            if !line_starts_in_string(text, line_idx) {
                let start_char = u32::from(line_idx == 0 && has_bom);
                let line_text_edit = TextEdit {
                    range: Range::new(
                        Position::new(diag.range.start.line, start_char),
                        Position::new(diag.range.start.line, start_char),
                    ),
                    new_text: format!("{indent}-- sqlsift:disable {code}{newline}"),
                };
                let mut line_changes = HashMap::new();
                line_changes.insert(uri.clone(), vec![line_text_edit]);

                let line_action = CodeAction {
                    title: format!("Disable {code} for this line"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(line_changes),
                        ..Default::default()
                    }),
                    is_preferred: Some(true),
                    ..Default::default()
                };
                actions.push(CodeActionOrCommand::CodeAction(line_action));
            }

            // Disable for this file
            let file_char = u32::from(has_bom);
            let file_text_edit = TextEdit {
                range: Range::new(Position::new(0, file_char), Position::new(0, file_char)),
                new_text: format!("-- sqlsift:disable-file {code}{newline}"),
            };
            let mut file_changes = HashMap::new();
            file_changes.insert(uri.clone(), vec![file_text_edit]);

            let file_action = CodeAction {
                title: format!("Disable {code} for this file"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(file_changes),
                    ..Default::default()
                }),
                is_preferred: Some(false),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(file_action));
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }
}

/// Whether line `line_idx` (0-indexed) starts inside a multi-line string literal.
fn line_starts_in_string(text: &str, line_idx: usize) -> bool {
    if line_idx == 0 {
        return false;
    }

    let mut target_offset = 0;
    let mut current_line = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            current_line += 1;
            if current_line == line_idx {
                target_offset = i + 1;
                break;
            }
        }
    }
    if current_line < line_idx {
        return false;
    }

    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if i >= target_offset {
            return false;
        }
        let rest = &text[i..];
        if rest.starts_with("--") {
            i += rest.find('\n').map_or(rest.len(), |n| n + 1);
        } else if let Some(body) = rest.strip_prefix("/*") {
            i += body.find("*/").map_or(rest.len(), |n| n + 4);
        } else if rest.starts_with('\'') {
            let start = i;
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'\'' {
                    if bytes.get(j + 1) == Some(&b'\'') {
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            if start < target_offset && target_offset < j {
                return true;
            }
            i = j;
        } else if let Some(end) = end_of_dollar_quoted(text, i) {
            let start = i;
            if start < target_offset && target_offset < end {
                return true;
            }
            i = end;
        } else if rest.starts_with('"') || rest.starts_with('`') {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == quote {
                    if bytes.get(j + 1) == Some(&quote) {
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            i = j;
        } else {
            let c = rest.chars().next().unwrap();
            i += c.len_utf8();
        }
    }
    false
}

/// End of the dollar-quoted string (`$$...$$`, `$tag$...$tag$`) starting at
/// `start`, or `None` if there is none there (e.g. a `$1` parameter)
fn end_of_dollar_quoted(sql: &str, start: usize) -> Option<usize> {
    let rest = sql.get(start..)?;
    if !rest.starts_with('$') {
        return None;
    }
    let tag_len = rest[1..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let tag_body = &rest[1..=tag_len];
    if tag_body.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !rest[1 + tag_len..].starts_with('$')
    {
        return None;
    }
    let delimiter = &rest[..tag_len + 2];
    let body_start = start + delimiter.len();
    Some(
        sql[body_start..]
            .find(delimiter)
            .map_or(sql.len(), |n| body_start + n + delimiter.len()),
    )
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

    #[test]
    fn test_line_starts_in_string() {
        let text = "SELECT 'a\nb', nme FROM users;";
        assert!(!line_starts_in_string(text, 0));
        assert!(line_starts_in_string(text, 1));

        let text2 = "SELECT 'hello' AS greeting,\n       nme FROM users;";
        assert!(!line_starts_in_string(text2, 0));
        assert!(!line_starts_in_string(text2, 1));

        let text3 = "-- comment with 'quote\nSELECT nme FROM users;";
        assert!(!line_starts_in_string(text3, 0));
        assert!(!line_starts_in_string(text3, 1));

        let text4 = "SELECT $$multi\nline$$ AS doc,\n       nme FROM users;";
        assert!(line_starts_in_string(text4, 1));
        assert!(!line_starts_in_string(text4, 2));
    }
}
