use std::sync::Arc;

use crate::documents::DocumentStore;
use crate::syntax;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticOptions,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentDiagnosticParams, DocumentDiagnosticReport, DocumentDiagnosticReportResult,
    FullDocumentDiagnosticReport, Hover, HoverContents, HoverParams, InitializeParams,
    InitializeResult, InitializedParams, MarkupContent, MarkupKind, MessageType,
    RelatedFullDocumentDiagnosticReport, ServerCapabilities, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp::{Client, LanguageServer};

pub struct Backend {
    client: Client,
    documents: Arc<DocumentStore>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(DocumentStore::default()),
        }
    }

    async fn publish_diagnostics(&self, uri: tower_lsp::lsp_types::Url, version: Option<i32>) {
        let diagnostics = self
            .documents
            .get(&uri)
            .and_then(|doc| syntax::parse(&doc.text).ok())
            .map(|parsed| parsed.diagnostics())
            .unwrap_or_default();
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    fn replace_text(changes: Vec<TextDocumentContentChangeEvent>) -> Option<String> {
        changes.into_iter().last().map(|change| change.text)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(tower_lsp::lsp_types::HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
                diagnostic_provider: Some(
                    tower_lsp::lsp_types::DiagnosticServerCapabilities::Options(
                        DiagnosticOptions {
                            identifier: Some("proverif-lsp-rs".into()),
                            inter_file_dependencies: false,
                            workspace_diagnostics: false,
                            work_done_progress_options: Default::default(),
                        },
                    ),
                ),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "proverif-lsp-rs initialized")
            .await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.documents.open(doc.uri.clone(), doc.version, doc.text);
        self.publish_diagnostics(doc.uri, Some(doc.version)).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        if let Some(text) = Self::replace_text(params.content_changes) {
            self.documents.update(&uri, version, text);
            self.publish_diagnostics(uri, Some(version)).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents.close(&params.text_document.uri);
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };

        let hover = syntax::parse(&doc.text)
            .ok()
            .and_then(|parsed| parsed.hover(position))
            .map(|value| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: None,
            });

        Ok(hover)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };

        let items = syntax::parse(&doc.text)
            .map(|parsed| parsed.completion_items(position))
            .unwrap_or_default();

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = params.text_document.uri;
        let diagnostics: Vec<Diagnostic> = self
            .documents
            .get(&uri)
            .and_then(|doc| syntax::parse(&doc.text).ok())
            .map(|parsed| parsed.diagnostics())
            .unwrap_or_default();

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items: diagnostics,
                },
            }),
        ))
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
