//! Module parsing.
//!
//! Parse a single source file (JS/TS/JSX/TSX) into an OXC `Program` AST
//! with semantic scoping from `SemanticBuilder`. Handles source type detection
//! from filename extension and reports parse errors as `Diagnostic` values.

use oxc::semantic::Scoping;

use crate::errors;
use crate::types::Diagnostic;

/// Result of parsing a single source file.
pub(crate) struct ParseResult<'a> {
    pub program: oxc::ast::ast::Program<'a>,
    pub source_type: oxc::span::SourceType,
    pub scoping: Scoping,
}

// Manual Debug impl because oxc::ast::ast::Program does not derive Debug
impl std::fmt::Debug for ParseResult<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseResult")
            .field("source_type", &self.source_type)
            .field("program.body.len", &self.program.body.len())
            .finish()
    }
}

/// Detect `SourceType` from a filename extension.
///
/// - `.tsx` -> TSX (TypeScript + JSX)
/// - `.ts`  -> TypeScript with JSX enabled (Qwik allows JSX in .ts files)
/// - `.jsx` -> JSX (JavaScript + JSX)
/// - `.js` / `.mjs` / `.cjs` -> ESM module (JavaScript)
/// - Default: ESM module
fn source_type_from_filename(filename: &str) -> oxc::span::SourceType {
    if filename.ends_with(".tsx") {
        oxc::span::SourceType::tsx()
    } else if filename.ends_with(".ts") {
        // Qwik allows JSX in .ts files, so enable JSX
        oxc::span::SourceType::ts().with_jsx(true)
    } else if filename.ends_with(".jsx") {
        oxc::span::SourceType::jsx()
    } else if filename.ends_with(".js") || filename.ends_with(".mjs") || filename.ends_with(".cjs")
    {
        oxc::span::SourceType::mjs()
    } else {
        oxc::span::SourceType::mjs()
    }
}

/// Parse a single source file into an OXC Program AST with semantic scoping.
///
/// The `source` must have lifetime `'a` tied to the allocator so the AST
/// can reference it. Returns `Err(diagnostics)` only if the parser panicked
/// (unrecoverable error with empty AST). For recoverable parse errors,
/// the partial AST is returned along with diagnostics (OXC guarantees a
/// structurally valid AST even with syntax errors when `panicked == false`).
pub(crate) fn parse_module<'a>(
    allocator: &'a oxc::allocator::Allocator,
    source: &'a str,
    filename: &str,
) -> Result<(ParseResult<'a>, Vec<Diagnostic>), Vec<Diagnostic>> {
    let source_type = source_type_from_filename(filename);

    // Parse source into AST
    let ret = oxc::parser::Parser::new(allocator, source, source_type).parse();

    // Only bail on unrecoverable parser panics (empty AST).
    // When panicked == false, OXC guarantees a structurally valid partial AST
    // even when there are syntax errors, so we can proceed with transformation.
    if ret.panicked {
        let diagnostics: Vec<Diagnostic> = ret
            .errors
            .iter()
            .map(|err| errors::create_source_error(&err.to_string(), filename))
            .collect();
        return Err(diagnostics);
    }

    // Collect any non-fatal parse errors as diagnostics
    let parse_diagnostics: Vec<Diagnostic> = ret
        .errors
        .iter()
        .map(|err| errors::create_source_error(&err.to_string(), filename))
        .collect();

    let program = ret.program;

    // Build semantic scoping
    let semantic_ret = oxc::semantic::SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .build(&program);
    let scoping = semantic_ret.semantic.into_scoping();

    Ok((
        ParseResult {
            program,
            source_type,
            scoping,
        },
        parse_diagnostics,
    ))
}
