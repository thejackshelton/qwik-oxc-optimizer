//! Diagnostic types and error helpers.
//!
//! Helper functions for creating `Diagnostic` values with consistent formatting.
//! Centralizes error message templates so the rest of the crate can report errors
//! without constructing Diagnostic structs manually.

use crate::types::{Diagnostic, DiagnosticCategory};

/// Create a source error diagnostic (e.g., syntax error).
pub(crate) fn create_source_error(message: &str, file: &str) -> Diagnostic {
    Diagnostic {
        scope: "optimizer".to_string(),
        category: DiagnosticCategory::SourceError,
        code: None,
        file: file.to_string(),
        message: message.to_string(),
        highlights: None,
        suggestions: None,
    }
}
