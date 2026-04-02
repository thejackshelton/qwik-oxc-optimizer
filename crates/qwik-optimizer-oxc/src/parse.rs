//! Module parsing.
//!
//! Parse a single source file (JS/TS/JSX/TSX) into an OXC `Program` AST
//! with semantic scoping from `SemanticBuilder`. Handles source type detection
//! from filename extension and reports parse errors as `Diagnostic` values.
//! Also provides path decomposition and output extension computation.

use std::path::{Path, PathBuf};

use oxc::semantic::Scoping;

use crate::errors;
use crate::types::Diagnostic;

/// Decomposed path data for a single input module.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PathData {
    /// Filename without extension (e.g., "index" for "src/routes/index.tsx").
    pub file_stem: String,

    /// Filename with extension (e.g., "index.tsx").
    pub file_name: String,

    /// Directory portion of the relative path (e.g., "src/routes").
    /// Empty PathBuf when the file is in the root.
    pub rel_dir: PathBuf,

    /// Absolute directory path = src_dir.join(rel_dir).
    pub abs_dir: PathBuf,
}

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
pub(crate) fn source_type_from_filename(filename: &str) -> oxc::span::SourceType {
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

/// Decompose a relative file path into its constituent parts.
///
/// `relative_path` is a slash-separated path relative to `src_dir`, e.g.
/// `"src/routes/index.tsx"`. `src_dir` is the absolute root directory.
///
/// Returns a `PathData` with:
/// - `file_stem`: filename without extension
/// - `file_name`: filename with extension
/// - `rel_dir`:   parent directory of relative_path (empty PathBuf when no parent)
/// - `abs_dir`:   `src_dir.join(rel_dir)`
pub(crate) fn parse_path(
    relative_path: &str,
    src_dir: &Path,
) -> Result<PathData, anyhow::Error> {
    // Normalize Windows-style path separators to forward slashes.
    // The input may contain backslashes (e.g., from Windows fixture configs) even
    // when running on macOS/Linux. Path::new() does not split on \ on non-Windows.
    let normalized_path = relative_path.replace('\\', "/");
    let rel_path = Path::new(&normalized_path);

    let file_name = rel_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {relative_path}"))?
        .to_string();

    let file_stem = rel_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("path has no file stem: {relative_path}"))?
        .to_string();

    // rel_dir is the parent of the relative path, or empty if there is none.
    let rel_dir = match rel_path.parent() {
        Some(p) if p != Path::new("") => p.to_path_buf(),
        _ => PathBuf::new(),
    };

    let abs_dir = src_dir.join(&rel_dir);

    Ok(PathData {
        file_stem,
        file_name,
        rel_dir,
        abs_dir,
    })
}

/// Map (transpile_ts, transpile_jsx, input extension) to the correct output extension.
///
/// Rules per SPEC.md §4 (lines 825-831):
/// - `.tsx` + transpile_ts + transpile_jsx -> `"js"`
/// - `.tsx` + transpile_ts + !transpile_jsx -> `"jsx"`
/// - `.tsx` + !transpile_ts + transpile_jsx -> `"ts"`  (strip JSX, keep TS)
/// - `.ts`  + transpile_ts  -> `"js"`
/// - `.jsx` + transpile_jsx -> `"js"`
/// - Everything else -> preserve original extension
pub(crate) fn output_extension(
    input_path: &str,
    transpile_ts: bool,
    transpile_jsx: bool,
) -> &'static str {
    let ext = input_path.rsplit('.').next().unwrap_or("");
    match ext {
        "tsx" => match (transpile_ts, transpile_jsx) {
            (true, true) => "js",
            (true, false) => "jsx",
            (false, true) => "ts",
            (false, false) => "tsx",
        },
        "ts" => {
            if transpile_ts {
                "js"
            } else {
                "ts"
            }
        }
        "jsx" => {
            if transpile_jsx {
                "js"
            } else {
                "jsx"
            }
        }
        "js" => "js",
        "mjs" => {
            // SWC normalizes .mjs to "js" only when transpiling (ts or jsx).
            // Without transpilation, preserve "mjs" (e.g. no-op passthrough of .mjs libs).
            if transpile_ts || transpile_jsx {
                "js"
            } else {
                "mjs"
            }
        }
        "cjs" => {
            // SWC normalizes .cjs to "js" only when transpiling.
            if transpile_ts || transpile_jsx {
                "js"
            } else {
                "cjs"
            }
        }
        _ => "js", // unknown: default to js
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

    // SWC behavior: recoverable (non-panicked) parse errors are silently ignored.
    // OXC guarantees a structurally valid partial AST when panicked == false, so
    // transformation proceeds normally. We do NOT collect these as diagnostics
    // to match SWC wire format (no sourceError entries for recoverable failures).
    let parse_diagnostics: Vec<Diagnostic> = vec![];

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oxc::allocator::Allocator;
    use std::path::Path;

    // ---- parse_module tests -----------------------------------------------

    #[test]
    fn test_parse_tsx_source() {
        let allocator = Allocator::default();
        let source = r#"import { component$ } from '@qwik.dev/core';
export const App = component$(() => {
    return <div>Hello</div>;
});"#;

        let result = parse_module(&allocator, source, "app.tsx");
        assert!(result.is_ok(), "Expected successful parse of TSX source");

        let (parsed, diags) = result.unwrap();
        assert!(diags.is_empty());
        assert!(parsed.source_type.is_typescript());
        assert!(parsed.source_type.is_jsx());
        assert!(!parsed.program.body.is_empty());
    }

    #[test]
    fn test_parse_ts_source() {
        let allocator = Allocator::default();
        let source = r#"const x: number = 42;"#;

        let result = parse_module(&allocator, source, "utils.ts");
        assert!(result.is_ok());

        let (parsed, _diags) = result.unwrap();
        assert!(parsed.source_type.is_typescript());
        // JSX is enabled for .ts in Qwik
        assert!(parsed.source_type.is_jsx());
    }

    #[test]
    fn test_parse_jsx_source() {
        let allocator = Allocator::default();
        let source = r#"export const App = () => <div>Hello</div>;"#;

        let result = parse_module(&allocator, source, "app.jsx");
        assert!(result.is_ok());

        let (parsed, _diags) = result.unwrap();
        assert!(!parsed.source_type.is_typescript());
        assert!(parsed.source_type.is_jsx());
    }

    #[test]
    fn test_parse_js_source() {
        let allocator = Allocator::default();
        let source = r#"export const x = 1;"#;

        let result = parse_module(&allocator, source, "utils.js");
        assert!(result.is_ok());

        let (parsed, _diags) = result.unwrap();
        assert!(!parsed.source_type.is_typescript());
    }

    #[test]
    fn test_parse_mjs_source() {
        let allocator = Allocator::default();
        let source = r#"export const x = 1;"#;

        let result = parse_module(&allocator, source, "utils.mjs");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_error_recovery() {
        let allocator = Allocator::default();
        // Syntax error: const without initializer, but recoverable
        let source = r#"const x = 1; const = ; const y = 2;"#;

        let result = parse_module(&allocator, source, "bad.tsx");
        // Should succeed with partial AST (recoverable error)
        // OR fail with panicked (unrecoverable) -- depends on OXC.
        // Recoverable errors produce empty diagnostics Vec (SWC behavior: silent recovery).
        match result {
            Ok((parsed, diags)) => {
                // After Fix 4: non-fatal parse errors produce no diagnostics (SWC behavior).
                assert!(diags.is_empty(), "Recoverable parse errors must produce empty diagnostics (SWC behavior), got: {:?}", diags);
                assert!(!parsed.program.body.is_empty(), "Expected partial AST");
            }
            Err(diags) => {
                assert!(!diags.is_empty());
            }
        }
    }

    #[test]
    fn test_parse_returns_scoping() {
        let allocator = Allocator::default();
        let source = r#"
import { $ } from '@qwik.dev/core';
const x = $(() => {
    const inner = 1;
    return inner;
});
"#;

        let result = parse_module(&allocator, source, "test.tsx");
        assert!(result.is_ok());
        let (parsed, _diags) = result.unwrap();
        let _scoping = &parsed.scoping;
    }

    #[test]
    fn test_source_type_detection() {
        assert!(source_type_from_filename("app.tsx").is_typescript());
        assert!(source_type_from_filename("app.tsx").is_jsx());

        assert!(source_type_from_filename("app.ts").is_typescript());
        assert!(source_type_from_filename("app.ts").is_jsx());

        assert!(!source_type_from_filename("app.jsx").is_typescript());
        assert!(source_type_from_filename("app.jsx").is_jsx());

        assert!(!source_type_from_filename("app.js").is_typescript());
        assert!(!source_type_from_filename("app.mjs").is_typescript());
        assert!(!source_type_from_filename("app.cjs").is_typescript());

        // Unknown extension defaults to mjs
        assert!(!source_type_from_filename("app.txt").is_typescript());
    }

    // ---- parse_path tests ------------------------------------------------

    #[test]
    fn test_parse_path_nested() {
        let src_dir = Path::new("/project");
        let result = parse_path("src/routes/index.tsx", src_dir).unwrap();
        assert_eq!(result.file_stem, "index");
        assert_eq!(result.file_name, "index.tsx");
        assert_eq!(result.rel_dir, PathBuf::from("src/routes"));
        assert_eq!(result.abs_dir, PathBuf::from("/project/src/routes"));
    }

    #[test]
    fn test_parse_path_root_level() {
        let src_dir = Path::new("/project");
        let result = parse_path("component.tsx", src_dir).unwrap();
        assert_eq!(result.file_stem, "component");
        assert_eq!(result.file_name, "component.tsx");
        assert_eq!(result.rel_dir, PathBuf::new());
        assert_eq!(result.abs_dir, PathBuf::from("/project"));
    }

    #[test]
    fn test_parse_path_one_level() {
        let src_dir = Path::new("/app");
        let result = parse_path("routes/index.ts", src_dir).unwrap();
        assert_eq!(result.file_stem, "index");
        assert_eq!(result.file_name, "index.ts");
        assert_eq!(result.rel_dir, PathBuf::from("routes"));
        assert_eq!(result.abs_dir, PathBuf::from("/app/routes"));
    }

    #[test]
    fn test_parse_path_windows_backslash() {
        let src_dir = Path::new("/src");
        let result = parse_path("components\\apps\\apps.tsx", src_dir).unwrap();
        assert_eq!(result.file_name, "apps.tsx");
        assert_eq!(result.file_stem, "apps");
        assert_eq!(result.rel_dir, PathBuf::from("components/apps"));
    }

    // ---- output_extension tests ------------------------------------------

    #[test]
    fn test_output_extension_tsx_both_transpile() {
        assert_eq!(output_extension("test.tsx", true, true), "js");
    }

    #[test]
    fn test_output_extension_tsx_ts_only() {
        assert_eq!(output_extension("test.tsx", true, false), "jsx");
    }

    #[test]
    fn test_output_extension_tsx_jsx_only() {
        assert_eq!(output_extension("test.tsx", false, true), "ts");
    }

    #[test]
    fn test_output_extension_tsx_no_transpile() {
        assert_eq!(output_extension("test.tsx", false, false), "tsx");
    }

    #[test]
    fn test_output_extension_ts_transpile_ts() {
        assert_eq!(output_extension("test.ts", true, true), "js");
    }

    #[test]
    fn test_output_extension_ts_no_transpile() {
        // transpile_jsx=true but file is .ts (not .tsx) -> no change
        assert_eq!(output_extension("test.ts", false, true), "ts");
    }

    #[test]
    fn test_output_extension_jsx_transpile_jsx() {
        assert_eq!(output_extension("test.jsx", false, true), "js");
    }

    #[test]
    fn test_output_extension_js_unchanged() {
        assert_eq!(output_extension("test.js", true, true), "js");
        assert_eq!(output_extension("test.js", false, false), "js");
    }

    #[test]
    fn test_output_extension_mjs_transpile_normalizes_to_js() {
        // SWC normalizes .mjs to "js" when transpiling (ts or jsx active)
        assert_eq!(output_extension("lib.mjs", true, true), "js");
        assert_eq!(output_extension("lib.mjs", true, false), "js");
        assert_eq!(output_extension("lib.mjs", false, true), "js");
    }

    #[test]
    fn test_output_extension_mjs_no_transpile_preserves() {
        // Without transpilation, .mjs stays "mjs" (e.g. passthrough of .mjs libs in no-transpile mode)
        assert_eq!(output_extension("lib.mjs", false, false), "mjs");
    }

    #[test]
    fn test_output_extension_cjs_transpile_normalizes_to_js() {
        // SWC normalizes .cjs to "js" when transpiling
        assert_eq!(output_extension("lib.cjs", true, true), "js");
        assert_eq!(output_extension("lib.cjs", true, false), "js");
        assert_eq!(output_extension("lib.cjs", false, true), "js");
    }

    #[test]
    fn test_output_extension_cjs_no_transpile_preserves() {
        // Without transpilation, .cjs stays "cjs"
        assert_eq!(output_extension("lib.cjs", false, false), "cjs");
    }
}
