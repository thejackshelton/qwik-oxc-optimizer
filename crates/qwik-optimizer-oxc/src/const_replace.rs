//! Build constant replacement — Stage 9.
//!
//! Replaces `isServer`, `isBrowser`, and `isDev` identifiers with boolean
//! literals based on the transform configuration. Imports from the following
//! sources are recognized:
//!
//! - `@qwik.dev/core`
//! - `@qwik.dev/core/build`
//!
//! **Denylist:** Replacement is skipped entirely for [`EmitMode::Lib`] and
//! [`EmitMode::Test`]. For these modes the identifiers remain untouched.
//!
//! Mapping:
//! - `isServer`  → `config.is_server`
//! - `isBrowser` → `!config.is_server`
//! - `isDev`     → `true` for Dev/Hmr modes, `false` otherwise

use std::collections::HashMap;

use oxc::allocator::Allocator;
use oxc::ast::AstBuilder;
use oxc::ast::ast::*;
use oxc::ast_visit::VisitMut;
use oxc::span::SPAN;

use crate::collector::GlobalCollect;
use crate::types::{EmitMode, TransformCodeOptions};

/// Module sources whose constants are eligible for replacement.
const BUILD_CONSTANT_SOURCES: &[&str] = &["@qwik.dev/core", "@qwik.dev/core/build"];

/// The recognized constant names.
const BUILD_CONSTANTS: &[&str] = &["isServer", "isBrowser", "isDev"];

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Replace build constants in `program` according to `config`.
///
/// - Skipped entirely for [`EmitMode::Lib`] and [`EmitMode::Test`].
/// - Uses `collect.imports` to discover which local binding names correspond
///   to the recognized build constants.
pub(crate) fn replace_build_constants<'a>(
    program: &mut Program<'a>,
    config: &TransformCodeOptions,
    collect: &GlobalCollect,
    allocator: &'a Allocator,
) {
    // Denylist gate: Lib and Test modes preserve the identifiers as-is.
    if config.mode == EmitMode::Lib || config.mode == EmitMode::Test {
        return;
    }

    let is_dev = matches!(config.mode, EmitMode::Dev | EmitMode::Hmr);

    // Build map: local_name -> replacement value.
    let mut replacements: HashMap<String, bool> = HashMap::new();
    for (local, import) in &collect.imports {
        if BUILD_CONSTANT_SOURCES
            .iter()
            .any(|&s| s == import.source.as_str())
        {
            let value = match import.specifier.as_str() {
                "isServer" => Some(config.is_server),
                "isBrowser" => Some(!config.is_server),
                "isDev" => Some(is_dev),
                _ => None,
            };
            if let Some(v) = value {
                replacements.insert(local.clone(), v);
            }
        }
    }

    if replacements.is_empty() {
        return;
    }

    let ast = AstBuilder::new(allocator);
    let mut replacer = ConstReplacer { replacements, ast };
    replacer.visit_program(program);
}

// ---------------------------------------------------------------------------
// ConstReplacer visitor
// ---------------------------------------------------------------------------

struct ConstReplacer<'a> {
    replacements: HashMap<String, bool>,
    ast: AstBuilder<'a>,
}

impl<'a> VisitMut<'a> for ConstReplacer<'a> {
    fn visit_expression(&mut self, expr: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expr {
            let name = ident.name.as_str();
            if let Some(&value) = self.replacements.get(name) {
                *expr = self.ast.expression_boolean_literal(SPAN, value);
                return;
            }
        }
        // Walk children for all other expression kinds.
        oxc::ast_visit::walk_mut::walk_expression(self, expr);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::global_collect;
    use crate::types::EmitMode;
    use oxc::allocator::Allocator;
    use oxc::codegen::Codegen;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    fn make_config(is_server: bool, mode: EmitMode) -> TransformCodeOptions {
        TransformCodeOptions {
            src_dir: "/project".to_string(),
            root_dir: None,
            source_maps: false,
            minify: crate::types::MinifyMode::None,
            transpile_ts: false,
            transpile_jsx: false,
            preserve_filenames: false,
            entry_strategy: Default::default(),
            explicit_extensions: false,
            mode,
            scope: None,
            core_module: "@qwik.dev/core".to_string(),
            strip_exports: vec![],
            strip_ctx_name: vec![],
            strip_event_handlers: false,
            reg_ctx_name: vec![],
            is_server,
        }
    }

    fn transform(src: &str, config: &TransformCodeOptions) -> String {
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let ret = Parser::new(&allocator, src, source_type).parse();
        let mut program = ret.program;
        let collect = global_collect(&program);
        replace_build_constants(&mut program, config, &collect, &allocator);
        Codegen::new().build(&program).code
    }

    // -----------------------------------------------------------------------
    // Basic replacement tests
    // -----------------------------------------------------------------------

    #[test]
    fn replaces_is_server_true() {
        // Stage 9 replaces usages of isServer; the import declaration itself is
        // stripped in a later stage. Verify console.log receives `true`.
        let src =
            r#"import { isServer } from "@qwik.dev/core/build"; console.log(isServer);"#;
        let out = transform(src, &make_config(true, EmitMode::Prod));
        assert!(
            out.contains("console.log(true)"),
            "Expected console.log(true) from isServer=true replacement, got: {out}"
        );
    }

    #[test]
    fn replaces_is_server_false() {
        let src =
            r#"import { isServer } from "@qwik.dev/core/build"; console.log(isServer);"#;
        let out = transform(src, &make_config(false, EmitMode::Prod));
        assert!(
            out.contains("console.log(false)"),
            "Expected console.log(false) from isServer=false replacement, got: {out}"
        );
    }

    #[test]
    fn replaces_is_browser_inverse_of_is_server() {
        let src =
            r#"import { isBrowser } from "@qwik.dev/core/build"; console.log(isBrowser);"#;
        // is_server=true → isBrowser=false
        let out = transform(src, &make_config(true, EmitMode::Prod));
        assert!(
            out.contains("console.log(false)"),
            "Expected console.log(false) when isBrowser+is_server=true, got: {out}"
        );

        // is_server=false → isBrowser=true
        let out2 = transform(src, &make_config(false, EmitMode::Prod));
        assert!(
            out2.contains("console.log(true)"),
            "Expected console.log(true) when isBrowser+is_server=false, got: {out2}"
        );
    }

    #[test]
    fn replaces_is_dev_true_for_dev_mode() {
        let src =
            r#"import { isDev } from "@qwik.dev/core/build"; console.log(isDev);"#;
        let out = transform(src, &make_config(false, EmitMode::Dev));
        assert!(
            out.contains("console.log(true)"),
            "Expected console.log(true) for isDev in Dev mode, got: {out}"
        );
    }

    #[test]
    fn replaces_is_dev_true_for_hmr_mode() {
        let src =
            r#"import { isDev } from "@qwik.dev/core/build"; console.log(isDev);"#;
        let out = transform(src, &make_config(false, EmitMode::Hmr));
        assert!(
            out.contains("true"),
            "Expected isDev=true for Hmr mode, got: {out}"
        );
    }

    #[test]
    fn replaces_is_dev_false_for_prod_mode() {
        let src =
            r#"import { isDev } from "@qwik.dev/core/build"; console.log(isDev);"#;
        let out = transform(src, &make_config(false, EmitMode::Prod));
        assert!(
            out.contains("false"),
            "Expected isDev=false for Prod mode, got: {out}"
        );
    }

    #[test]
    fn also_recognizes_core_module_without_build_suffix() {
        // "@qwik.dev/core" (not /build) should also be recognized.
        let src = r#"import { isServer } from "@qwik.dev/core"; console.log(isServer);"#;
        let out = transform(src, &make_config(true, EmitMode::Prod));
        assert!(
            out.contains("console.log(true)"),
            "Expected console.log(true) from @qwik.dev/core isServer replacement, got: {out}"
        );
    }

    // -----------------------------------------------------------------------
    // Denylist tests
    // -----------------------------------------------------------------------

    #[test]
    fn skips_replacement_for_lib_mode() {
        let src =
            r#"import { isServer } from "@qwik.dev/core/build"; console.log(isServer);"#;
        let out = transform(src, &make_config(true, EmitMode::Lib));
        // Identifier must be preserved in the call expression.
        assert!(
            out.contains("console.log(isServer)"),
            "isServer must be preserved in call for Lib mode, got: {out}"
        );
    }

    #[test]
    fn skips_replacement_for_test_mode() {
        let src =
            r#"import { isServer } from "@qwik.dev/core/build"; console.log(isServer);"#;
        let out = transform(src, &make_config(true, EmitMode::Test));
        assert!(
            out.contains("console.log(isServer)"),
            "isServer must be preserved in call for Test mode, got: {out}"
        );
    }

    // -----------------------------------------------------------------------
    // Non-constant import not replaced
    // -----------------------------------------------------------------------

    #[test]
    fn does_not_replace_non_constant_import() {
        // `x` is not one of isServer/isBrowser/isDev — must not be touched.
        let src =
            r#"import { x } from "@qwik.dev/core/build"; console.log(x);"#;
        let out = transform(src, &make_config(true, EmitMode::Prod));
        assert!(
            out.contains("x"),
            "Non-constant binding 'x' must not be replaced, got: {out}"
        );
    }
}
