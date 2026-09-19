use lsp_types::{self as types, request as req};
use types::{
    DocumentDiagnosticReportResult, FullDocumentDiagnosticReport,
    RelatedFullDocumentDiagnosticReport,
};

use crate::lint::generate_available_diagnostics;
use crate::server::Result;
use crate::session::{Client, DocumentSnapshot};

pub(crate) struct DocumentDiagnostic;

impl super::RequestHandler for DocumentDiagnostic {
    type RequestType = req::DocumentDiagnosticRequest;
}

impl super::BackgroundDocumentRequestHandler for DocumentDiagnostic {
    super::define_document_url!(params: &types::DocumentDiagnosticParams);

    fn run_without_snapshot(
        _client: &Client,
        _params: types::DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        Ok(DocumentDiagnosticReportResult::Report(
            types::DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items: Vec::new(),
                },
            }),
        ))
    }

    fn run_with_snapshot(
        snapshot: DocumentSnapshot,
        _client: &Client,
        params: types::DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        use sha2::{Digest, Sha256};
        let items = generate_available_diagnostics(&snapshot);
        let bytes = serde_json::to_vec(&items).unwrap_or_default();
        let result_id = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let report = if params.previous_result_id.as_ref() == Some(&result_id) {
            types::DocumentDiagnosticReport::Unchanged(
                types::RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report:
                        types::UnchangedDocumentDiagnosticReport { result_id },
                },
            )
        } else {
            types::DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items,
                },
            })
        };
        Ok(DocumentDiagnosticReportResult::Report(report))
    }
}
