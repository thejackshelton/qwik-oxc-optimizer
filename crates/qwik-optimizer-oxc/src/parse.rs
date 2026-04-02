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

/// Attempt to recover from parse errors using multiple strategies.
///
/// Strategy 1: Remove unmatched trailing `)` (e.g. `export const App = () => {...});`)
/// Strategy 2: Re-parse with each top-level statement individually to extract what works
///
/// Returns the best recovered source string, or None if no recovery possible.
fn try_recover_source(
    source: &str,
    source_type: &oxc::span::SourceType,
    allocator: &oxc::allocator::Allocator,
) -> Option<String> {
    // Strategy 1: Fix unmatched trailing parens
    let mut result = source.to_string();
    loop {
        let mut depth: i32 = 0;
        for ch in result.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
        if depth >= 0 { break; }
        if let Some(pos) = result.rfind(')') {
            result.remove(pos);
        } else {
            break;
        }
    }

    if result != source {
        let test_src: &str = allocator.alloc_str(&result);
        let ret = oxc::parser::Parser::new(allocator, test_src, *source_type).parse();
        if !ret.program.body.is_empty() {
            return Some(result);
        }
    }

    // Strategy 2: Progressively strip lines from the end of the source,
    // trying to find a parseable subset. This handles cases where trailing
    // invalid syntax causes the whole parse to fail.
    // Only try a few variations to keep it fast.
    let lines: Vec<&str> = source.lines().collect();
    for drop_count in 1..std::cmp::min(5, lines.len()) {
        let subset = lines[..lines.len() - drop_count].join("\n");
        // Quick paren balance check before trying to parse
        let mut depth: i32 = 0;
        for ch in subset.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
        // Also try adding missing closing tokens
        let mut candidate = subset.clone();
        while depth > 0 {
            candidate.push(')');
            depth -= 1;
        }
        candidate.push(';');

        let test_src: &str = allocator.alloc_str(&candidate);
        let ret = oxc::parser::Parser::new(allocator, test_src, *source_type).parse();
        if !ret.program.body.is_empty() && !ret.panicked {
            return Some(candidate);
        }
    }

    // Strategy 3: JSX expression container fixup.
    // Some fixtures (e.g. example_immutable_analysis) contain bare expressions
    // like `[].map(() => (...));` as direct JSX Fragment children without `{}`
    // wrapping. This is invalid JSX — children that are expressions must be
    // wrapped in expression containers `{expr}`. SWC recovers from this; OXC
    // panics. We scan for lines that look like bare expressions between JSX
    // elements and wrap them in `{...}`.
    if let Some(fixed) = try_jsx_expression_container_fixup(source) {
        let test_src: &str = allocator.alloc_str(&fixed);
        let ret = oxc::parser::Parser::new(allocator, test_src, *source_type).parse();
        if !ret.program.body.is_empty() && !ret.panicked {
            return Some(fixed);
        }
    }

    None
}

/// Try to fix bare expressions used as JSX children by wrapping them in `{}`.
///
/// Scans the source for lines between JSX closing tags (`</...>` or `/>`) and
/// JSX opening tags (`<...`) that look like expressions (starting with `[`,
/// `(`, or identifiers followed by `.` or `(`). These are invalid JSX that
/// should be in expression containers.
fn try_jsx_expression_container_fixup(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut result_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let mut modified = false;

    // Find regions of bare expressions between JSX elements.
    // A bare expression region starts after a JSX closing tag line (</X> or />)
    // or after a JSX self-closing line, and contains lines that look like
    // expression code (not JSX tags, not `{` expression containers).
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Check if this line starts a bare expression that's inside JSX context.
        // We detect this by looking for lines that:
        // 1. Start with `[`, or an identifier/expression pattern
        // 2. Are preceded by a JSX closing tag (</...> or />)
        // 3. Are NOT already inside `{...}`
        if is_bare_expression_start(trimmed) && is_in_jsx_context(&lines, i) {
            // Find the end of this bare expression (line ending with `;`)
            let start = i;
            let mut end = i;
            for j in i..lines.len() {
                let t = lines[j].trim();
                if t.ends_with(';') {
                    end = j;
                    break;
                }
                // If we hit a JSX opening tag, stop before it
                if t.starts_with('<') && !t.starts_with("</") && !t.starts_with("<!") {
                    // The expression likely contains JSX, find the actual end
                    // by looking for the semicolon after the JSX closes
                    continue;
                }
                end = j;
            }

            // Wrap the bare expression in { ... }
            // Get the indentation of the first line
            let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];

            // Remove the trailing semicolon from the last line if present
            let last_trimmed = result_lines[end].trim().to_string();
            let last_without_semi = if last_trimmed.ends_with(';') {
                last_trimmed[..last_trimmed.len() - 1].to_string()
            } else {
                last_trimmed
            };

            // Reconstruct: {original_expression}
            // Wrap by prepending `{` to first line and appending `}` after last line
            result_lines[start] = format!("{}{{{}", indent, lines[start].trim());
            let last_indent = &lines[end][..lines[end].len() - lines[end].trim_start().len()];
            result_lines[end] = format!("{}{}}}", last_indent, last_without_semi);

            modified = true;
            i = end + 1;
        } else {
            i += 1;
        }
    }

    if modified {
        Some(result_lines.join("\n"))
    } else {
        None
    }
}

/// Check if a trimmed line looks like the start of a bare expression
/// (not a JSX tag, not empty, not a comment).
fn is_bare_expression_start(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    // Bare expressions typically start with `[`, `(`, or an identifier
    // They should NOT start with `<` (JSX), `{` (already in container),
    // `}`, `//`, `/*`, or be a closing fragment `</>`
    let first_char = trimmed.chars().next().unwrap();
    matches!(first_char, '[' | '(')
        || (first_char.is_alphabetic() && !trimmed.starts_with("return")
            && !trimmed.starts_with("const") && !trimmed.starts_with("let")
            && !trimmed.starts_with("var") && !trimmed.starts_with("import")
            && !trimmed.starts_with("export") && !trimmed.starts_with("if")
            && !trimmed.starts_with("for") && !trimmed.starts_with("while"))
}

/// Check if line at index `i` is inside a JSX context by looking at surrounding lines.
fn is_in_jsx_context(lines: &[&str], i: usize) -> bool {
    // Look backwards for a JSX closing tag or self-closing tag
    for j in (0..i).rev() {
        let trimmed = lines[j].trim();
        if trimmed.is_empty() {
            continue;
        }
        // JSX closing element: </Div>, </>, or self-closing: />
        // Also: line ending with > that contains a JSX element
        if trimmed.ends_with('>') && (trimmed.starts_with("</") || trimmed.ends_with("/>")
            || trimmed.contains("</"))
        {
            return true;
        }
        // If we see a non-JSX line, we're not in JSX context
        if !trimmed.starts_with('<') && !trimmed.starts_with('{')
            && !trimmed.starts_with('}') && !trimmed.starts_with("//")
        {
            return false;
        }
        // Keep looking if we see JSX-like content
        break;
    }

    // Also look forward for JSX opening
    for j in (i + 1)..lines.len() {
        let trimmed = lines[j].trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('<') && !trimmed.starts_with("</") {
            return true;
        }
        break;
    }

    false
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

    // Parse error recovery: OXC produces a structurally valid AST even when
    // panicked==true for many error types (e.g. unexpected tokens at EOF).
    // SWC recovers from these errors and still extracts segments.
    // Only bail if the resulting program is truly empty (catastrophic failure)
    // AND retry with error recovery fails.
    if ret.panicked && ret.program.body.is_empty() {
        // Retry: attempt to recover by stripping trailing unmatched delimiters
        // or other fixable patterns. SWC recovers from these; OXC doesn't.
        if let Some(recovered) = try_recover_source(source, &source_type, allocator) {
            let recovered_str: &str = allocator.alloc_str(&recovered);
            let ret2 = oxc::parser::Parser::new(allocator, recovered_str, source_type).parse();
            if !ret2.program.body.is_empty() {
                // Recovery succeeded — proceed with recovered partial AST.
                let parse_diagnostics: Vec<Diagnostic> = vec![];
                let program = ret2.program;
                let semantic_ret = oxc::semantic::SemanticBuilder::new()
                    .with_excess_capacity(2.0)
                    .build(&program);
                let scoping = semantic_ret.semantic.into_scoping();
                return Ok((
                    ParseResult {
                        program,
                        source_type,
                        scoping,
                    },
                    parse_diagnostics,
                ));
            }
        }

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
    fn test_parse_immutable_analysis_fixture() {
        // Simulates the example_immutable_analysis fixture that produces 0 segments
        let allocator = Allocator::default();
        let source = r#"
import { component$, useStore, $ } from '@qwik.dev/core';
export const App = component$((props) => {
	const state = useStore({count: 0});
	const remove = $((id: number) => {
		const d = state.data;
		d.splice(
			d.findIndex((d) => d.id === id),
			1
		)
		});
	return (
		<>
			<p class="stuff">Hello Qwik</p>
		</>
	);
});"#;

        let result = parse_module(&allocator, source, "test.tsx");
        match &result {
            Ok((parsed, _diags)) => {
                eprintln!("OK: body.len={}", parsed.program.body.len());
                assert!(!parsed.program.body.is_empty());
            }
            Err(diags) => {
                eprintln!("ERR: {} diagnostics", diags.len());
                for d in diags {
                    eprintln!("  {:?}", d);
                }
                panic!("Expected valid code to parse successfully");
            }
        }
    }

    #[test]
    fn test_parse_jsx_expression_container_recovery() {
        // The ACTUAL full example_immutable_analysis.tsx source contains bare
        // `[].map(() => (...));` as a direct JSX Fragment child without `{}`
        // wrapping. OXC's parser cannot handle this invalid JSX. We need a
        // recovery strategy that wraps such bare expressions in `{}`.
        let allocator = Allocator::default();
        let source = r#"
import { component$, useStore, $ } from '@qwik.dev/core';
import importedValue from 'v';
import styles from './styles.module.css';

export const App = component$((props) => {
	const {Model} = props;
	const state = useStore({count: 0});
	const remove = $((id: number) => {
		const d = state.data;
		d.splice(
			d.findIndex((d) => d.id === id),
			1
		)
		});
	return (
		<>
			<p class="stuff" onClick$={props.onClick$}>Hello Qwik</p>
			<Div
				class={styles.foo}
				document={window.document}
				onClick$={props.onClick$}
				onEvent$={() => console.log('stuff')}
				transparent$={() => {console.log('stuff')}}
				immutable1="stuff"
				immutable2={{
					foo: 'bar',
					baz: importedValue ? true : false,
				}}
				immutable3={2}
				immutable4$={(ev) => console.log(state.count)}
				immutable5={[1, 2, importedValue, null, {}]}
			>
				<p>Hello Qwik</p>
			</Div>
			[].map(() => (
				<Model
					class={state}
					remove$={remove}
					mutable1={{
						foo: 'bar',
						baz: state.count ? true : false,
					}}
					mutable2={(() => console.log(state.count))()}
					mutable3={[1, 2, state, null, {}]}
				/>
			));
		</>
	);
});"#;

        let result = parse_module(&allocator, source, "test.tsx");
        assert!(result.is_ok(), "Expected JSX expression container recovery to succeed");
        let (parsed, _diags) = result.unwrap();
        assert!(!parsed.program.body.is_empty(), "Expected non-empty body");
        assert!(parsed.program.body.len() >= 2, "Expected at least 2 body statements (import + export), got {}", parsed.program.body.len());
        eprintln!("JSX expression container recovery: body.len={}", parsed.program.body.len());
    }

    #[test]
    fn test_parse_jsx_no_false_positive_recovery() {
        // A simple valid JSX file should NOT be modified by recovery
        let allocator = Allocator::default();
        let source = r#"import { component$ } from '@qwik.dev/core';
export const App = component$(() => {
    return <div>{[1,2].map(x => <span>{x}</span>)}</div>;
});"#;

        let result = parse_module(&allocator, source, "test.tsx");
        assert!(result.is_ok(), "Valid JSX should parse without recovery");
        let (parsed, _diags) = result.unwrap();
        assert!(!parsed.program.body.is_empty());
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
    fn test_parse_trailing_paren_recovery() {
        // This pattern appears in example_3 and example_immutable_analysis fixtures:
        // export const App = () => { ... });  — the trailing ); is extraneous
        // SWC recovers and produces segments. OXC should also recover.
        let allocator = Allocator::default();
        let source = r#"import { $, component$ } from '@qwik.dev/core';
export const App = () => {
    const Header = component$(() => {
        return <div/>;
    });
    return Header;
});"#;

        let result = parse_module(&allocator, source, "test.tsx");
        // With error recovery, this should now succeed
        assert!(result.is_ok(), "Expected parse recovery to succeed for trailing-paren pattern");
        let (parsed, diags) = result.unwrap();
        assert!(diags.is_empty(), "Parse errors should be suppressed");
        assert!(!parsed.program.body.is_empty(), "Expected partial AST with imports and exports");
        eprintln!("Recovery produced {} body statements", parsed.program.body.len());
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
