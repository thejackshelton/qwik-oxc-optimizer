//! Dependency analysis for variable migration (Stage 12).
//!
//! This module implements the first four steps of the 10-step variable
//! migration pipeline (SPEC lines 2884-3110):
//!
//! 1. `analyze_root_dependencies` — extract root-level var/fn/class declarations
//!    with their dependencies and import/export status.
//! 2. `build_root_var_usage_map` — which segments reference which root vars.
//! 3. `build_main_module_usage_set` — vars still used by the root module's
//!    non-segment runtime items.
//! 4. `find_migratable_vars` — 4-condition check + safety fixpoint loop.

use std::collections::{BTreeMap, HashMap, HashSet};

use oxc::ast::ast::*;
use oxc::ast_visit::Visit;
use oxc::span::GetSpan;

use crate::collector::GlobalCollect;
use crate::transform::SegmentRecord;

// ---------------------------------------------------------------------------
// RootVarInfo
// ---------------------------------------------------------------------------

/// Metadata about a single root-level declaration in the parent module.
#[derive(Debug, Clone)]
pub(crate) struct RootVarInfo {
    /// Full declaration code string (e.g. `"const THRESHOLD = 100;"`).
    pub code: String,
    /// True when the name is in `GlobalCollect.imports` (already imported from
    /// another module — must not be migrated).
    pub is_imported: bool,
    /// True only for *real* exports.  `_auto_` prefixed re-export aliases are
    /// excluded per SPEC Pitfall 7 — those are auto-generated and safe to remove.
    pub is_exported: bool,
    /// Identifiers referenced in the initializer / body that are also declared
    /// at the root level.
    pub depends_on: Vec<String>,
}

// ---------------------------------------------------------------------------
// analyze_root_dependencies
// ---------------------------------------------------------------------------

/// Parse `root_code` and return a map of `var_name -> RootVarInfo` for every
/// top-level `var`/`const`/`let`/`function`/`class` declaration.
///
/// The `global` argument supplies import/export membership checks.
pub(crate) fn analyze_root_dependencies(
    root_code: &str,
    global: &GlobalCollect,
) -> HashMap<String, RootVarInfo> {
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let src: &str = allocator.alloc_str(root_code);
    let ret = Parser::new(&allocator, src, SourceType::mjs()).parse();
    if ret.panicked {
        return HashMap::new();
    }
    let program = ret.program;

    // Collect the set of all root-level declaration names first so we can
    // filter depends_on to only intra-root references.
    let mut all_root_names: HashSet<String> = HashSet::new();
    for stmt in &program.body {
        collect_decl_names_stmt(stmt, &mut all_root_names);
    }

    // Build the set of *real* export names (exported_name strings), excluding
    // any that start with `_auto_`.
    // In practice the code_move pipeline uses `_auto_<sym>` for all
    // ensure_export injections; real user exports keep their own name.
    let real_export_locals: HashSet<String> = global
        .exports
        .keys()
        .filter(|k| !k.starts_with("_auto_"))
        .cloned()
        .collect();

    let mut result: HashMap<String, RootVarInfo> = HashMap::new();

    for stmt in &program.body {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                process_var_decl(decl, src, &all_root_names, global, &real_export_locals, &mut result, false);
            }
            Statement::FunctionDeclaration(fn_decl) => {
                let name = match fn_decl.id.as_ref() {
                    Some(id) => id.name.to_string(),
                    None => continue,
                };
                let code = span_to_str(src, fn_decl.span);
                let depends_on = match &fn_decl.body {
                    Some(body) => {
                        let mut c = IdentRefCollector::default();
                        c.visit_function_body(body);
                        c.names.into_iter().filter(|n| all_root_names.contains(n)).collect()
                    }
                    None => Vec::new(),
                };
                result.insert(
                    name.clone(),
                    RootVarInfo {
                        code,
                        is_imported: global.imports.contains_key(&name),
                        is_exported: real_export_locals.contains(&name),
                        depends_on,
                    },
                );
            }
            Statement::ClassDeclaration(cls) => {
                let name = match cls.id.as_ref() {
                    Some(id) => id.name.to_string(),
                    None => continue,
                };
                let code = span_to_str(src, cls.span);
                let depends_on = {
                    let mut c = IdentRefCollector::default();
                    c.visit_class(cls);
                    c.names.into_iter().filter(|n| all_root_names.contains(n)).collect()
                };
                result.insert(
                    name.clone(),
                    RootVarInfo {
                        code,
                        is_imported: global.imports.contains_key(&name),
                        is_exported: real_export_locals.contains(&name),
                        depends_on,
                    },
                );
            }
            Statement::ExportNamedDeclaration(export_decl) => {
                if let Some(decl) = &export_decl.declaration {
                    match decl {
                        Declaration::VariableDeclaration(var_decl) => {
                            process_var_decl(var_decl, src, &all_root_names, global, &real_export_locals, &mut result, true);
                        }
                        Declaration::FunctionDeclaration(fn_decl) => {
                            if let Some(id) = &fn_decl.id {
                                let name = id.name.to_string();
                                let code = span_to_str(src, export_decl.span);
                                let depends_on = match &fn_decl.body {
                                    Some(body) => {
                                        let mut c = IdentRefCollector::default();
                                        c.visit_function_body(body);
                                        c.names.into_iter().filter(|n| all_root_names.contains(n)).collect()
                                    }
                                    None => Vec::new(),
                                };
                                result.insert(
                                    name.clone(),
                                    RootVarInfo {
                                        code,
                                        is_imported: global.imports.contains_key(&name),
                                        is_exported: true,
                                        depends_on,
                                    },
                                );
                            }
                        }
                        Declaration::ClassDeclaration(cls) => {
                            if let Some(id) = &cls.id {
                                let name = id.name.to_string();
                                let code = span_to_str(src, export_decl.span);
                                let depends_on = {
                                    let mut c = IdentRefCollector::default();
                                    c.visit_class(cls);
                                    c.names.into_iter().filter(|n| all_root_names.contains(n)).collect()
                                };
                                result.insert(
                                    name.clone(),
                                    RootVarInfo {
                                        code,
                                        is_imported: global.imports.contains_key(&name),
                                        is_exported: true,
                                        depends_on,
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    result
}

/// Process a `VariableDeclaration` node and insert entries into `result`.
fn process_var_decl<'a>(
    decl: &VariableDeclaration<'a>,
    src: &str,
    all_root_names: &HashSet<String>,
    global: &GlobalCollect,
    real_export_locals: &HashSet<String>,
    result: &mut HashMap<String, RootVarInfo>,
    force_exported: bool,
) {
    let kind_str = match decl.kind {
        VariableDeclarationKind::Const => "const",
        VariableDeclarationKind::Let => "let",
        VariableDeclarationKind::Var => "var",
        VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing => "const",
    };
    for declarator in &decl.declarations {
        let name = match &declarator.id {
            BindingPattern::BindingIdentifier(id) => id.name.to_string(),
            _ => continue,
        };
        // Build code string: "const name = <init>;" or "const name;"
        let code = if let Some(init) = &declarator.init {
            let init_code = span_to_str(src, init.span());
            if init_code.is_empty() {
                format!("{} {};", kind_str, name)
            } else {
                format!("{} {} = {};", kind_str, name, init_code)
            }
        } else {
            format!("{} {};", kind_str, name)
        };

        let depends_on = if let Some(init) = &declarator.init {
            let mut c = IdentRefCollector::default();
            c.visit_expression(init);
            c.names.into_iter().filter(|n| all_root_names.contains(n)).collect()
        } else {
            Vec::new()
        };

        let is_exported = force_exported || real_export_locals.contains(&name);

        result.insert(
            name.clone(),
            RootVarInfo {
                code,
                is_imported: global.imports.contains_key(&name),
                is_exported,
                depends_on,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// build_root_var_usage_map
// ---------------------------------------------------------------------------

/// For each root var, find which segments (by index into `segments`) reference it.
///
/// "References" means the var name appears either:
///  - In `segment.local_idents`, or
///  - As a whole-word match in `segment.expr` code.
///
/// Returns `HashMap<var_name, Vec<segment_index>>`.
pub(crate) fn build_root_var_usage_map(
    root_deps: &HashMap<String, RootVarInfo>,
    segments: &[SegmentRecord],
) -> HashMap<String, Vec<usize>> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();

    for var_name in root_deps.keys() {
        let mut seg_indices: Vec<usize> = Vec::new();

        for (idx, seg) in segments.iter().enumerate() {
            let referenced_in_local = seg.local_idents.contains(var_name);
            let referenced_in_expr = seg.expr.as_ref().map_or(false, |expr_code| {
                contains_whole_word(expr_code, var_name)
            });
            if referenced_in_local || referenced_in_expr {
                seg_indices.push(idx);
            }
        }

        map.insert(var_name.clone(), seg_indices);
    }

    map
}

// ---------------------------------------------------------------------------
// build_main_module_usage_set
// ---------------------------------------------------------------------------

/// Return the set of root-level var names still referenced by the root module's
/// *non-segment* code.
///
/// Strategy: parse `root_code`, collect all identifier references that appear
/// outside of QRL-related call expression statements (componentQrl, useTaskQrl,
/// etc.).  These are the names the root module needs at runtime — they must not
/// be migrated even if a segment uses them.
///
/// "QRL call statements" are top-level `ExpressionStatement`s whose callee name
/// ends in `Qrl` or `QrlDEV`.
pub(crate) fn build_main_module_usage_set(
    root_code: &str,
    _segments: &[SegmentRecord],
) -> HashSet<String> {
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let src: &str = allocator.alloc_str(root_code);
    let ret = Parser::new(&allocator, src, SourceType::mjs()).parse();
    if ret.panicked {
        return HashSet::new();
    }
    let program = ret.program;

    let mut usage: HashSet<String> = HashSet::new();

    for stmt in &program.body {
        // Skip top-level expression statements that are QRL calls.
        let is_qrl_call_stmt = match stmt {
            Statement::ExpressionStatement(es) => match &es.expression {
                Expression::CallExpression(call) => match &call.callee {
                    Expression::Identifier(id) => {
                        let name = id.name.as_str();
                        name.ends_with("Qrl") || name.ends_with("QrlDEV")
                    }
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        };

        if !is_qrl_call_stmt {
            // Collect all identifier references in this statement.
            let mut collector = IdentRefCollector::default();
            collector.visit_statement(stmt);
            for name in collector.names {
                usage.insert(name);
            }
        }
    }

    usage
}

// ---------------------------------------------------------------------------
// find_migratable_vars
// ---------------------------------------------------------------------------

/// Apply the 4-condition check + safety fixpoint to determine which vars can
/// be migrated into which segments.
///
/// Returns `BTreeMap<segment_index, Vec<var_name>>` (deterministic order).
///
/// The 4 conditions for a var `v` to be a migration candidate:
///   1. `!v.is_imported`
///   2. `!v.is_exported` (real exports only; `_auto_` aliases are fine to remove)
///   3. `usage_map[v].len() == 1` (used by exactly one segment)
///   4. `v` not in `main_usage` (not needed by root module runtime code)
///
/// Safety fixpoint: after initial candidate set is built, iteratively remove
/// any candidate var whose `depends_on` list contains a name that is either:
///  (a) not itself a candidate, or
///  (b) a candidate but assigned to a *different* segment.
/// Repeat until stable.
pub(crate) fn find_migratable_vars(
    root_deps: &HashMap<String, RootVarInfo>,
    usage_map: &HashMap<String, Vec<usize>>,
    main_usage: &HashSet<String>,
) -> BTreeMap<usize, Vec<String>> {
    // Initial candidates: var_name -> segment_index
    let mut candidates: HashMap<String, usize> = HashMap::new();

    for (var_name, info) in root_deps {
        // Condition 1: not imported
        if info.is_imported {
            continue;
        }
        // Condition 2: not a real export
        if info.is_exported {
            continue;
        }
        // Condition 3: used by exactly one segment
        let seg_usages = match usage_map.get(var_name) {
            Some(v) => v,
            None => continue,
        };
        if seg_usages.len() != 1 {
            continue;
        }
        // Condition 4: not in main_usage
        if main_usage.contains(var_name) {
            continue;
        }

        candidates.insert(var_name.clone(), seg_usages[0]);
    }

    // Safety fixpoint: remove vars whose deps are not all migratable to the same segment.
    loop {
        let mut removed: Vec<String> = Vec::new();

        for (var_name, seg_idx) in &candidates {
            let info = match root_deps.get(var_name) {
                Some(i) => i,
                None => {
                    removed.push(var_name.clone());
                    continue;
                }
            };

            for dep in &info.depends_on {
                // Is this dep itself a candidate?
                match candidates.get(dep) {
                    Some(&dep_seg) if dep_seg == *seg_idx => {
                        // Dep is also migratable to the same segment — OK.
                    }
                    Some(_) => {
                        // Dep is in a different segment — unsafe.
                        removed.push(var_name.clone());
                        break;
                    }
                    None => {
                        // Dep is not a candidate at all.
                        // It must either be an import (available everywhere) or a
                        // root symbol that stays with a real export so the segment
                        // can import it.
                        let dep_info = root_deps.get(dep);
                        let dep_is_safe = dep_info.map_or(true, |di| {
                            di.is_imported || di.is_exported
                        });
                        if !dep_is_safe {
                            removed.push(var_name.clone());
                            break;
                        }
                    }
                }
            }
        }

        if removed.is_empty() {
            break;
        }
        for name in removed {
            candidates.remove(&name);
        }
    }

    // Group by segment index.
    let mut result: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (var_name, seg_idx) in candidates {
        result.entry(seg_idx).or_default().push(var_name);
    }
    // Sort var names within each segment for determinism.
    for names in result.values_mut() {
        names.sort();
    }

    result
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Collect top-level declaration names from a statement.
fn collect_decl_names_stmt(stmt: &Statement<'_>, out: &mut HashSet<String>) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let BindingPattern::BindingIdentifier(id) = &d.id {
                    out.insert(id.name.to_string());
                }
            }
        }
        Statement::FunctionDeclaration(fn_decl) => {
            if let Some(id) = &fn_decl.id {
                out.insert(id.name.to_string());
            }
        }
        Statement::ClassDeclaration(cls) => {
            if let Some(id) = &cls.id {
                out.insert(id.name.to_string());
            }
        }
        Statement::ExportNamedDeclaration(export_decl) => {
            if let Some(decl) = &export_decl.declaration {
                match decl {
                    Declaration::VariableDeclaration(var_decl) => {
                        for d in &var_decl.declarations {
                            if let BindingPattern::BindingIdentifier(id) = &d.id {
                                out.insert(id.name.to_string());
                            }
                        }
                    }
                    Declaration::FunctionDeclaration(fn_decl) => {
                        if let Some(id) = &fn_decl.id {
                            out.insert(id.name.to_string());
                        }
                    }
                    Declaration::ClassDeclaration(cls) => {
                        if let Some(id) = &cls.id {
                            out.insert(id.name.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Extract source text for a span.
fn span_to_str(src: &str, span: oxc::span::Span) -> String {
    let start = span.start as usize;
    let end = span.end as usize;
    if start <= end && end <= src.len() {
        src[start..end].to_string()
    } else {
        String::new()
    }
}

/// Check whether `text` contains `word` as a whole-word match.
pub(crate) fn contains_whole_word(text: &str, word: &str) -> bool {
    let wlen = word.len();
    let tlen = text.len();
    if wlen > tlen {
        return false;
    }
    let bytes = text.as_bytes();
    let wbytes = word.as_bytes();
    let mut start = 0;
    while start + wlen <= tlen {
        if bytes[start..start + wlen] == *wbytes {
            let pre_ok = start == 0 || !is_ident_char(bytes[start - 1]);
            let post_ok = start + wlen == tlen || !is_ident_char(bytes[start + wlen]);
            if pre_ok && post_ok {
                return true;
            }
        }
        start += 1;
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

// ---------------------------------------------------------------------------
// IdentRefCollector — visitor that accumulates all IdentifierReference names
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct IdentRefCollector {
    pub names: Vec<String>,
}

impl<'a> Visit<'a> for IdentRefCollector {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.names.push(ident.name.to_string());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::global_collect_from_str;
    use crate::transform::SegmentRecord;
    use crate::types::CtxKind;

    fn empty_global() -> GlobalCollect {
        crate::collector::GlobalCollect::new_empty()
    }

    fn make_segment(name: &str, local_idents: Vec<&str>, expr: Option<&str>) -> SegmentRecord {
        SegmentRecord {
            name: name.to_string(),
            display_name: name.to_string(),
            canonical_filename: name.to_string(),
            entry: None,
            expr: expr.map(|s| s.to_string()),
            scoped_idents: Vec::new(),
            local_idents: local_idents.into_iter().map(|s| s.to_string()).collect(),
            ctx_name: "component$".to_string(),
            ctx_kind: CtxKind::Function,
            origin: "test.tsx".to_string(),
            span: (0, 0),
            hash: "abc12345678".to_string(),
            is_inline: false,
            migrated_root_vars: Vec::new(),
            parent: None,
            param_names: None,
            pending_parent_span: None,
        }
    }

    // -----------------------------------------------------------------------
    // analyze_root_dependencies
    // -----------------------------------------------------------------------

    #[test]
    fn analyze_root_dependencies_finds_const_decl() {
        let code = "const THRESHOLD = 100;";
        let global = empty_global();
        let result = analyze_root_dependencies(code, &global);
        assert!(result.contains_key("THRESHOLD"), "should find THRESHOLD");
        let info = &result["THRESHOLD"];
        assert!(!info.is_imported);
        assert!(!info.is_exported);
        assert!(info.depends_on.is_empty());
    }

    #[test]
    fn analyze_root_dependencies_marks_imported() {
        let code = r#"import { helper } from "./utils"; const helper2 = helper + 1;"#;
        let global = global_collect_from_str(code);
        let result = analyze_root_dependencies(code, &global);
        // helper2 is a root const
        if let Some(info) = result.get("helper2") {
            assert!(!info.is_imported, "helper2 is not an import");
        }
        // helper is an import binding — if it appears as a root decl, it must be marked imported
        if let Some(info) = result.get("helper") {
            assert!(info.is_imported, "helper is imported");
        }
    }

    #[test]
    fn analyze_root_dependencies_marks_real_export() {
        let code = "export const FOO = 42;";
        let global = global_collect_from_str(code);
        let result = analyze_root_dependencies(code, &global);
        if let Some(info) = result.get("FOO") {
            assert!(info.is_exported, "FOO should be marked as exported");
        }
    }

    #[test]
    fn analyze_root_dependencies_excludes_auto_export() {
        // _auto_ exports should NOT cause is_exported = true.
        let code = "const THRESHOLD = 100;";
        let global = {
            let mut g = empty_global();
            g.exports.insert("_auto_THRESHOLD".to_string(), Default::default());
            g
        };
        let result = analyze_root_dependencies(code, &global);
        let info = &result["THRESHOLD"];
        assert!(!info.is_exported, "_auto_ export should NOT set is_exported");
    }

    #[test]
    fn analyze_root_dependencies_depends_on_tracks_intra_root_refs() {
        let code = "const A = 1;\nconst B = A + 2;";
        let global = empty_global();
        let result = analyze_root_dependencies(code, &global);
        assert!(result.contains_key("A"), "should find A");
        assert!(result.contains_key("B"), "should find B");
        let b_info = &result["B"];
        assert!(
            b_info.depends_on.contains(&"A".to_string()),
            "B depends on A: {:?}",
            b_info.depends_on
        );
    }

    // -----------------------------------------------------------------------
    // build_root_var_usage_map
    // -----------------------------------------------------------------------

    #[test]
    fn build_root_var_usage_map_finds_segment_using_var() {
        let code = "const THRESHOLD = 100;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);

        let segments = vec![make_segment(
            "s_abc",
            vec!["THRESHOLD"],
            Some("() => THRESHOLD > 50"),
        )];
        let map = build_root_var_usage_map(&root_deps, &segments);
        let usages = map.get("THRESHOLD").expect("THRESHOLD should be in map");
        assert_eq!(usages, &[0usize], "segment 0 uses THRESHOLD");
    }

    #[test]
    fn build_root_var_usage_map_shared_var_gets_two_segments() {
        let code = "const SHARED = 42;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);

        let segments = vec![
            make_segment("s_a", vec!["SHARED"], Some("() => SHARED")),
            make_segment("s_b", vec!["SHARED"], Some("() => SHARED * 2")),
        ];
        let map = build_root_var_usage_map(&root_deps, &segments);
        let usages = map.get("SHARED").expect("SHARED should be in map");
        assert_eq!(usages.len(), 2, "two segments use SHARED");
    }

    // -----------------------------------------------------------------------
    // build_main_module_usage_set
    // -----------------------------------------------------------------------

    #[test]
    fn build_main_module_usage_set_includes_non_qrl_refs() {
        let code = "const THRESHOLD = 100;\nif (THRESHOLD > 50) { console.log(\"ok\"); }";
        let segments: Vec<SegmentRecord> = Vec::new();
        let usage = build_main_module_usage_set(code, &segments);
        assert!(
            usage.contains("THRESHOLD"),
            "THRESHOLD is referenced in non-QRL if statement"
        );
    }

    #[test]
    fn build_main_module_usage_set_excludes_qrl_call_refs() {
        // A variable only referenced inside a componentQrl() call stmt
        // should not appear in the main usage set.
        let code = "const ONLY_IN_QRL = 42;\ncomponentQrl(ONLY_IN_QRL);";
        let segments: Vec<SegmentRecord> = Vec::new();
        let usage = build_main_module_usage_set(code, &segments);
        assert!(
            !usage.contains("ONLY_IN_QRL"),
            "ONLY_IN_QRL should be excluded (only in QRL call)"
        );
    }

    // -----------------------------------------------------------------------
    // find_migratable_vars
    // -----------------------------------------------------------------------

    #[test]
    fn find_migratable_vars_basic_single_segment() {
        let code = "const THRESHOLD = 100;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);
        let segments = vec![make_segment("s_a", vec!["THRESHOLD"], Some("() => THRESHOLD"))];
        let usage_map = build_root_var_usage_map(&root_deps, &segments);
        let main_usage = HashSet::new();
        let result = find_migratable_vars(&root_deps, &usage_map, &main_usage);
        assert!(result.contains_key(&0), "segment 0 should receive THRESHOLD");
        assert!(result[&0].contains(&"THRESHOLD".to_string()));
    }

    #[test]
    fn find_migratable_vars_shared_var_not_migrated() {
        let code = "const SHARED = 42;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);
        let segments = vec![
            make_segment("s_a", vec!["SHARED"], Some("() => SHARED")),
            make_segment("s_b", vec!["SHARED"], Some("() => SHARED * 2")),
        ];
        let usage_map = build_root_var_usage_map(&root_deps, &segments);
        let main_usage = HashSet::new();
        let result = find_migratable_vars(&root_deps, &usage_map, &main_usage);
        for vars in result.values() {
            assert!(!vars.contains(&"SHARED".to_string()), "SHARED must not be migrated");
        }
    }

    #[test]
    fn find_migratable_vars_main_usage_blocks_migration() {
        let code = "const THRESHOLD = 100;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);
        let segments = vec![make_segment("s_a", vec!["THRESHOLD"], Some("() => THRESHOLD"))];
        let usage_map = build_root_var_usage_map(&root_deps, &segments);
        let mut main_usage = HashSet::new();
        main_usage.insert("THRESHOLD".to_string());
        let result = find_migratable_vars(&root_deps, &usage_map, &main_usage);
        for vars in result.values() {
            assert!(
                !vars.contains(&"THRESHOLD".to_string()),
                "THRESHOLD is in main_usage — must not migrate"
            );
        }
    }

    #[test]
    fn find_migratable_vars_exported_not_migrated() {
        let code = "export const PUBLIC = 99;";
        let global = global_collect_from_str(code);
        let root_deps = analyze_root_dependencies(code, &global);
        let segments = vec![make_segment("s_a", vec!["PUBLIC"], Some("() => PUBLIC"))];
        let usage_map = build_root_var_usage_map(&root_deps, &segments);
        let main_usage = HashSet::new();
        let result = find_migratable_vars(&root_deps, &usage_map, &main_usage);
        for vars in result.values() {
            assert!(
                !vars.contains(&"PUBLIC".to_string()),
                "exported PUBLIC must not be migrated"
            );
        }
    }

    #[test]
    fn find_migratable_vars_safety_fixpoint_removes_dep_chain() {
        // A is used by 2 segments, B is used by 1 segment but depends on A.
        // Since A is not migratable (shared) and A is not exported/imported,
        // B cannot be migrated either (fixpoint removes it).
        let code = "const A = 1;\nconst B = A + 2;";
        let global = empty_global();
        let root_deps = analyze_root_dependencies(code, &global);
        // A used by 2 segments, B only by 1.
        let segments = vec![
            make_segment("s_a", vec!["A", "B"], Some("() => A + B")),
            make_segment("s_b", vec!["A"], Some("() => A * 3")),
        ];
        let usage_map = build_root_var_usage_map(&root_deps, &segments);
        let main_usage = HashSet::new();
        let result = find_migratable_vars(&root_deps, &usage_map, &main_usage);
        // A is shared — not migrated.
        // B depends on A which is not migratable and not exported — B also not migrated.
        for vars in result.values() {
            assert!(!vars.contains(&"B".to_string()), "B should be removed by fixpoint");
        }
    }

    // -----------------------------------------------------------------------
    // contains_whole_word
    // -----------------------------------------------------------------------

    #[test]
    fn contains_whole_word_basic() {
        assert!(contains_whole_word("THRESHOLD > 50", "THRESHOLD"));
        assert!(!contains_whole_word("THRESHOLD > 50", "THRESH"));
        assert!(!contains_whole_word("MY_THRESHOLD > 50", "THRESHOLD"));
        assert!(contains_whole_word("x = THRESHOLD;", "THRESHOLD"));
    }
}
