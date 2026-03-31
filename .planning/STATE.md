# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-19)

**Core value:** Semantic/behavioral parity with the SWC optimizer across all 162 test cases. Purely aesthetic diffs (OXC codegen shorthand, line wrapping, whitespace) are accepted.
**Current focus:** Phase 14 COMPLETE. 86 exact matches (85 from Plans 01-03 + 1 new from Plan 04 C05 diagnostic). Phases 15-18 optional.

## Current Position

Phase: 14 of 18 (Final Parity)
Plan: 4 of 4 in phase 14 (ALL COMPLETE)
Status: Phase complete
Last activity: 2026-02-24 - Completed 14-04-PLAN.md

Progress: [████████████████████████████████] 41/? plans

## Performance Metrics

**Velocity:**
- Total plans completed: 41
- Average duration: 19min
- Total execution time: 16.90 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 01-naming | 2/2 | 60min | 30min |
| 02-metadata | 1/1 | 15min | 15min |
| 03-bugs-correctness | 3/3 | 51min | 17min |
| 04-signal-props-transforms | 4/4 | 77min | 19min |
| 05-jsx-keys-flags | 3/3 | 36min | 12min |
| 06-import-ordering-cleanup | 3/3 | 39min | 13min |
| 07-entry-module-emission | 1/1 | 3min | 3min |
| 08-jsx-flags-iteration-variables | 3/3 | 41min | 14min |
| 09-jsx-keys-final-parity | 5/5 | 328min | 66min |
| 10-captures-mechanism | 1/1 | 13min | 13min |
| 11-auto-export-rename | 1/1 | 13min | 13min |
| 12-signal-wrapping-gaps | 6/6 | 72min | 12min |
| 13-captures-dce | 4/4 | 79min | 20min |
| 14-final-parity | 4/4 | 194min | 49min |

**Recent Trend:**
- Last 5 plans: 14-01 (45min), 14-03 (19min), 14-02 (75min), 14-04 (55min)
- Trend: 14-04 required SWC investigation for JSXProp classification logic. Two sessions due to context exhaustion.

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [Roadmap]: 6-phase cascade -- naming first (clears most noise), imports last (final cleanup after all correct imports exist)
- [Roadmap]: ISSUES.md priority ordering adopted -- naming cascades into everything, bugs before features (missing segments block testing)
- [01-01]: Combined stack_ctxt + JSX event handler naming into single architectural change since they share the same push/pop mechanism
- [01-01]: OXC represents component JSX elements as IdentifierReference (not Identifier) -- must handle both variants
- [01-01]: Kept dollar_call_stack alongside new segment_stack for backward compatibility with finalize_segments matching
- [01-02]: Hash computation: hash on display_name WITHOUT filename prefix, then prepend file_name after (matches SWC lines 358-368)
- [01-02]: Fragment naming: only push when transpile_jsx=true (SWC sees Fragment after JSX transform)
- [01-02]: Raw $() calls: don't push callee name (SWC's handle_qsegment returns before push)
- [01-02]: Prod mode: use s_HASH for segment names in EmitMode::Prod
- [01-cleanup]: Collector display name derivation removed entirely (DollarCallSite, derive_display_name, etc.) -- was dead code never consumed by transform
- [02-01]: 23 remaining paramNames mismatches deferred to Phase 4 -- q:p iteration variable injection (22) + useResource$ _rawProps (1) are transform issues, not metadata extraction
- [02-01]: OXC FormalParameterRest has nested .rest.argument path (different from plan assumption)
- [02-01]: support_windows_paths test had double backslashes vs SWC's single -- fixed
- [03-01]: Extra args passthrough applies to ALL named $-suffixed calls generically, not just component$
- [03-01]: OXC Codegen::print_expression() doesn't call build_comments() -- used temporary Program + build() for comment-preserving codegen instead
- [03-02]: JsxOptions::disable() required -- OXC TransformOptions default enables JSX plugin which would convert JSX to React format
- [03-02]: Scoping rebuild after transformer: SemanticBuilder::new().with_excess_capacity(2.0).build(&program)
- [03-03]: Segment sort key: span.0 (source byte offset) not display_name -- matches SWC fold top-down source order
- [03-03]: Capture ordering: SWC uses HashSet->Vec->sort() producing alphabetical order; added capture_names.sort() in compute_captures()
- [03-03]: should_extract_single_qrl_2 dedup suffix naming issue deferred -- bottom-up traverse assigns _1 to wrong segment
- [04-01]: Local variables are co-reactive in _fnSignal: they become deps only when primary reactive sources (signal.value, _rawProps, store chains) exist in same expression
- [04-01]: OXC codegen parentheses stripped from _fnSignal string representation to match SWC format
- [04-02]: in_callback_depth uses u32 counter (not bool) for nested iteration methods (.map inside .map)
- [04-02]: SWC uses "_" for both placeholder params (positions 0 and 1), not "_" and "_1"
- [04-02]: q:p/q:ps keys use string literal format in var_props (colon requires quoting)
- [04-02]: QRL hoisting deferred to Phase 6 -- OXC Traverse hoisted_function_stmts only injects at module top level
- [04-03]: Default expressions serialized to strings during analysis, rebuilt via parse-and-clone during rewrite (avoids arena lifetime issues)
- [04-03]: Import identifiers treated as const for default value checking (matches SWC is_const_expr)
- [04-03]: use*() return value destructuring inlining deferred to Phase 6 (only 2 test fixtures affected)
- [04-04]: Body destructuring detected separately from parameter destructuring -- two distinct code paths
- [04-04]: Props param name threaded through all JSX transform functions as Option<&str> parameter
- [04-04]: Prop alias origin mapping for _fnSignal: test.value -> [props] dep with p0.test.value hoisted fn
- [04-04]: argument_to_expression was missing MemberExpression variants (pre-existing bug) -- fixed
- [05-01]: Manual base64url encoding (6-bit lookup table) instead of adding base64 crate dependency
- [05-01]: root_jsx_mode hooks added to all 9 SWC-equivalent statement types (function, arrow, for/for-in/for-of, while, do-while, if, block, return)
- [05-01]: is_fn detection: uppercase first char on Identifier/IdentifierReference + MemberExpression match
- [05-02]: immutable_function_cmp built in QwikTransform::new() from collected imports (Fragment, RenderOnce, Link, ?jsx/.md)
- [05-02]: jsx_mutable and immutable_function_cmp stored on ImportTracker for jsx_transform.rs access
- [05-02]: WrapPropSignal keeps immutable (SWC is_const=true); WrapPropNamed marks mutable (SWC is_const=false)
- [05-02]: Identifiers/member exprs treated as immutable in children (approximates SWC scope; may miss globals)
- [05-03]: OXC bottom-up traversal requires pre-capture of tracker.jsx_mutable before save/restore in child processing
- [05-03]: contains_mutable_jsx_call scans expression trees for _jsxSorted calls with non-immutable component tags
- [05-03]: Member expressions: mutable by default, immutable only when base object is a known import (matches SWC ConstCollector)
- [05-03]: Remaining 21 flag mismatches are scope-analysis issues (unresolved globals, local mutable bindings)
- [06-02]: _captures import emitted first (before sorted list) -- SWC special case, not sorted with other imports
- [06-02]: Standard Rust string comparison for import sort order -- matches SWC Atom::cmp
- [06-02]: Lazy import declarations moved after all sorted imports -- matches SWC extra_top_items positioning
- [06-01]: Post-hoc referenced-ident filtering in exit_program instead of scope-tracking during traversal (conceptually matches SWC DCE)
- [06-01]: collect_referenced_idents descends into nested function/arrow bodies (correct for inline strategy)
- [06-01]: JSX element names need dedicated walker (JSXIdentifier/JSXElementName separate from Expression::Identifier)
- [06-01]: BTreeMap grouping for specifier merging preserves insertion order (matches SWC original specifier order)
- [06-01]: Side-effect imports (no specifiers) always kept regardless of reference scanning
- [06-03]: Flush QRL hoists at function/arrow exit when loop_depth == 0 (avoids flushing inside .map() callback arrows)
- [06-03]: Insert hoisted const declarations after variable declarations at function body top (matches SWC positioning)
- [06-03]: Forward iteration order for hoists matches SWC BTreeMap alphabetical ordering
- [06-03]: Lazy imports filtered by referenced-ident analysis in exit_program (critical for correct entry module with hoisted QRLs)
- [07-01]: New is_inline_like_strategy variable before main_code block gates _hf* injection (segment strategy skips entry module injection)
- [07-01]: body_code.contains(var_name) for per-segment _hf* filtering -- extracts var name from "const _hfN = ..." via strip_prefix + split
- [07-01]: _fnSignal import: removed || !hoisted_stmts.is_empty() proxy -- body_code.contains("_fnSignal") is sufficient alone
- [08-01]: const_bindings populated from imports at init + from const declarations via enter_variable_declaration hook
- [08-01]: is_const_expression_with_scope is fully recursive for compound expressions (binary, conditional, template literal, etc.)
- [08-01]: Member expressions in children use const_bindings instead of module_imports scan for consistency
- [08-02]: q:p injection moved to replace_jsx_element_handlers for correct element-level injection (not top-level via transform_jsx_element_inner)
- [08-02]: Deep ident scan (analyze_lambda_deep_ident_refs) descends into nested functions for iteration variable detection
- [08-02]: iter_var_usage_by_handler keyed by lambda span start enables per-handler tracking
- [08-02]: useResource$ gets full props destructuring rewrite (param + body), not just parameter replacement
- [08-02]: Body-level destructuring detection gated to component$ only
- [08-02]: Child segment capture reclassification gated to component$ only
- [08-03]: static_subtree NOT affected by var_props presence -- SWC only uses spread + children_mutable
- [08-03]: Event handler values classified by is_const_event_handler: qrl/inlinedQrl = const, _qrlSync/serverQrl = non-const
- [08-03]: q:p/q:ps always go to var_props regardless of const_bindings scope
- [08-03]: static_listeners false when q-e:* keys exist in var_props (non-const event handlers)
- [08-03]: 33 remaining flag mismatches all caused by upstream prop/transform differences, not flag computation bugs
- [09-01]: Pure flag check for DCE: only drop unused var decls with CallExpression.pure=true init (not all call inits)
- [09-01]: Export specifier references collected by collect_referenced_idents to prevent incorrect DCE
- [09-01]: EntryStrategy::Hook mapped same as Segment (returns None for entry field)
- [09-01]: JSX event rename in exit_expression AFTER segment extraction (not in exit_jsx_attribute which breaks extraction)
- [09-02]: Generic local variable wrapping uses const_bindings (not full scope analysis) to identify known local declarations for _wrapProp
- [09-02]: is_text_only elements (title, textarea, script, etc.) skip all signal wrapping and mark children as mutable
- [09-02]: WrapPropNamed(String, bool) carries is_const flag: true for const locals, false for props/_rawProps
- [09-02]: Entry module synthetic import ordering left as encounter-order mismatch (only 1 purely import-order diff remaining)
- [09-02]: Locally-defined Qrl functions reclassified from segment_qrl_names to needed_imports as self-imports
- [09-03]: body_span uses first argument span (arrow fn), not call expression span -- matches SWC's first_arg.span()
- [09-03]: SWC BytePos is 1-based; OXC spans are 0-based -- add 1 to lo/hi for dev metadata parity
- [09-03]: Function/class declarations tracked in invalid_decl_stack, excluded from captures, emit C02 diagnostics
- [09-03]: Diagnostic field order matches SWC: category, code, file, message, highlights, suggestions, scope
- [09-03]: Dev mode file path: dev_abs_path() = src_dir + "/" + filename; test config differences accepted as non-code-bug
- [09-04]: SWC handle_jsx saves/restores root_jsx_mode (not just sets false) -- OXC needs enter/exit_jsx_element save/restore
- [09-04]: SWC key counter assignment is bottom-up (children get lower counter values than parent) -- OXC bottom-up exit_expression naturally matches
- [09-04]: SWC fold_cond_expr/fold_bin_expr set root_jsx_mode=true -- OXC needs enter_conditional_expression and enter_logical_expression hooks
- [09-04]: Import assertions stored as Vec<(String, String)> key-value pairs threaded through ImportInfo -> ReemittedImport -> SegmentImportEntry
- [09-05]: Member expressions (StaticMemberExpression, ComputedMemberExpression) and call expressions always return false for is_const_expression_with_scope (matches SWC ConstCollector)
- [09-05]: _fnSignal deps constness determines prop placement: all_deps_const -> const_props, else -> var_props
- [09-05]: Event handler merging via merge_or_add_to_props: deduplicates q-e:input handlers into array expressions
- [09-05]: Mixed reactive+import deps in children marked mutable when collect_reactive_deps returns deps + has_non_reactive
- [09-05]: _wrapProp root constness: function params are non-const (SWC Var(false)), useSignal/useStore results are const
- [09-05]: _jsxSplit: all explicit props go into var_props object in source order; const_props classification irrelevant for explicit attrs
- [09-05]: Multi-spread _jsxSplit: _getConstProps inlined as spread, const_props arg null, explicit props between spreads ordered before remaining spread args
- [09-05]: TS type assertion lookahead: unwrap TSAsExpression, TSSatisfiesExpression, TSNonNullExpression, ParenthesizedExpression for signal wrapping
- [09-05]: Sync QRL strings minified via CodegenOptions { minify: true } + post-processing for semicolons and outer parens
- [09-05]: OXC codegen shorthand behavior: ignores shorthand=false flag, auto-converts {key: value} to {key} when names match -- fundamental OXC limitation
- [10-01]: current_iteration_vars() uses iteration_var_stack.last() (innermost loop only), matching SWC -- outer loop vars become captures
- [10-01]: paramNames metadata keeps duplicate "_" (SWC stores ["_", "_", "row"]); de-duplication to _1 only in code generation
- [10-01]: inject_iteration_params() runs before inject_captures_into_body() to avoid arrow position shift issues
- [11-01]: export default function/class treated as exported for _auto_ purposes (matches SWC)
- [11-01]: Destructured pattern exports tracked via collect_binding_pattern_names_into (not just simple bindings)
- [11-01]: Stripped segments excluded from auto_exports via is_stripped parameter
- [11-01]: TS enum _auto_ exports accepted as known OXC limitation (SWC inlines enum values)
- [12-01]: Dep sorting: primary_deps.sort_by root_name AFTER local_deps merge, re-assign pN param names after sort
- [12-01]: Direct CallExpression = side effect (has_non_reactive_non_const = true), ChainExpression calls = NOT side effect (allows signal.formData?.get() wrapping)
- [12-01]: Harmless globals (undefined, NaN, Infinity) don't block wrapping -- classified separately from other KNOWN_GLOBALS
- [12-01]: Props path: removed contains_function_call gate; children path retains it (matches SWC accept_call_expr=true vs false)
- [12-01]: TaggedTemplateExpression unconditionally sets has_non_reactive in dep collection AND added to contains_function_call
- [12-02]: is_any_dep_used_as_object gate checks if dep is used as member expression object before _fnSignal wrapping (matches SWC is_used_as_object_or_call)
- [12-02]: is_dep_or_contains_dep sees through LogicalExpression and ParenthesizedExpression for (a||b).value patterns
- [12-02]: collect_all_idents_as_primary_deps handles .value on non-identifier-chain expressions (e.g., (count||count2).value)
- [12-02]: Children path: deps exist but no dep used as object -> mark mutable without _fnSignal wrapping
- [12-03]: _hf dedup key is full arrow source "(p0) => p0.errors.test" -- params + body distinguishes different arities
- [12-03]: Caller-side dedup via hoisted_stmts.iter().any() -- simpler than return-flag, works at both props and children call sites
- [12-03]: HashMap<String, u32> on ImportTracker leverages #[derive(Default)] for automatic empty-map init
- [12-04]: When _rawProps dep comes from destructured prop alias detection (destructured_props non-empty), bypass is_any_dep_used_as_object check entirely
- [12-04]: Object key position detected by looking for { or , before identifier and : after it (heuristic, conservative)
- [12-04]: Scope resolution :: excluded from object key detection to avoid false positives
- [12-05]: const_bindings used as scope-analysis proxy for reactive dep detection in collect_reactive_deps
- [12-05]: Depth >= 2 chains bypass const_bindings check (unambiguously store chains)
- [12-05]: Depth 1 chains require const_bindings membership (prevents false positives on free variables like state.thing)
- [12-06]: Inline component detected via ExportDefaultDeclarationKind::ArrowFunctionExpression (no TS wrapper needed since TS stripping runs first)
- [12-06]: Capture name remapping done early in create_jsx_event_segments_recursive (not exit_export_default_declaration) to avoid timing issues
- [12-06]: Segment body post-processing done inline at body capture time (not deferred to exit hook)
- [12-06]: has_destructured_raw_props bypass extended: covers both _rawProps and named props params (body destructuring case)
- [12-06]: Identifier branch in collect_reactive_deps_inner uses props_param_name.unwrap_or("_rawProps") instead of hardcoded _rawProps
- [12-06]: Hoisted _hf* functions injected into entry module when entry code references var_name (enables segment strategy inline components)
- [13-01]: Plan premise was wrong: SWC captures _rawProps (not individual props). Existing reclassification direction was correct.
- [13-01]: Nested function/arrow params tracked as body_local_decls in enter hooks to prevent capture leaks
- [13-01]: Segment body codes post-processed after reclassification to replace prop aliases with _rawProps.propName
- [13-01]: QRL capture arrays rebuilt via fix_qrl_captures_in_body after reclassification
- [13-01]: Const literal inlining only in child segments (not parent component body) -- DCE deferred
- [13-01]: Const literal filtering also applies to reemitted_imports when local const shadows module import
- [13-01]: ArrayExpression elements in props_destructuring handled via index-based iteration (array_element_as_expression_mut was broken)
- [13-02]: Non-top-level $() captures filtered by all_parent_decls || module_level_decls (excludes unresolved identifiers like `children`)
- [13-02]: C03 diagnostic emitted for non-function $() arguments that capture local identifiers (clears captures)
- [13-02]: Hoist strategy segment extraction deferred -- requires Hoist infrastructure, not capture changes
- [13-02]: _fnSignal wrapping for `results[i]` in loops is Phase 12 scope, not capture issue
- [13-03]: Combined Task 1 (DCE) and Task 2 (invalid_decl removal) into single commit since force_remove_names is integral to apply_segment_body_dce signature
- [13-03]: Conservative destructuring DCE: keep destructuring patterns when init is not a simple identifier (safer than SWC which removes them)
- [13-03]: example_props_optimization constant-folding sub-issue documented as pre-existing signal wrapping difference, not DCE
- [13-03]: JSX reference collection uses as_expression() for JSXExpression (OXC inherit_variants macro flattens Expression into JSXExpression)
- [13-04]: isBrowser/isServer DCE already working in inline strategy via const_replace VisitMut recursion -- no code changes needed
- [13-04]: Const literal propagation deferred to Phase 14 (minor 2-5 line diffs per test)
- [13-04]: Destructured const chain folding deferred to Phase 14 (complex SWC MinifyMode::Simplify feature)
- [14-01]: SWC BTreeMap<Id> uses SyntaxContext (encounter order) -- replaced alphabetical sorting with encounter-order Vec
- [14-01]: componentQrl recorded in enter_call_expression (SWC encounters callee first), inlinedQrl/qrl deferred to exit_expression (SWC wraps after folding body)
- [14-01]: Fragment transform_jsx_fragment_inner defers _jsxSorted recording to after transform_jsx_children -- lets child elements record at correct position
- [14-01]: _getVarProps/_getConstProps recorded before _jsxSplit (SWC processes call arguments before the call itself)
- [14-01]: synthetic_import_count tracks framework import count for lib.rs hoisted _hf* stmt injection positioning
- [14-03]: SWC PerSegmentStrategy returns None (not entry_segments) -- corrected 3 incorrect golden snapshots
- [14-03]: Smart strategy: event handlers without captures get None via scoped_idents.is_empty() check matching SWC
- [14-03]: is_entry = entry.is_none() matching SWC parse.rs line 393 -- fixes Single/Smart strategy segment module classification
- [14-03]: noop_dev_mode uses /hello/from/dev/ src_dir matching SWC dev_path override, other dev tests use /user/qwik/src/
- [14-02]: Single-spread const_props algorithm: 3 cases based on (has_explicit_const && has_var_after) || var_after_has_fn_signal
- [14-02]: const_before stays in 2nd arg (var_props object), const_after may go to 3rd depending on var_after presence
- [14-02]: const_after entries referencing spread source reclassified to var_after (prevents double-counting)
- [14-02]: _createElement detection: single spread + user key + spread source NOT component's props param
- [14-02]: code_move.rs body_code.contains() extended for _createElement segment import emission
- [14-04]: JSXProp classification uses element type (native vs component) not attribute name -- matches SWC transpile_jsx:true code path
- [14-04]: C02 diagnostics keep highlights:null (matches SWC golden); only C03 gets highlight spans
- [14-04]: C05 emitted in enter_call_expression for exported $-suffixed calls missing Qrl counterpart
- [14-04]: Highlight span formula: lo/hi = OXC offset + 1, startCol = 0-based col + 1, endCol = 0-based col at exclusive end (no +1)

### Pending Todos

Phase 14 COMPLETE. All 4 plans executed.
- Phase 15: Signal wrapping & JSX flags (~20 tests)
- Phase 16: DCE & captures (~25 tests)
- Phase 17: Hoist strategy (14 tests, architectural)
- Phase 18: JSX import & remaining (~18 tests)
Phases 15-18 are optional depending on shipping needs.

### Blockers/Concerns

- BUG-01 (TS stripping) RESOLVED -- oxc_transformer with TypeScript-only config, JSX explicitly disabled
- BUG-02 (component options) RESOLVED -- extra argument passthrough for all named $-suffixed calls
- BUG-03 (capture ordering) RESOLVED -- alphabetical sort in compute_captures()
- BUG-04 (segment ordering) RESOLVED -- span-based sort before output iteration
- BUG-05 (test fixture) RESOLVED -- real 1074-line qwik-router bundle
- BUG-06 (source comments) RESOLVED -- temporary Program + build() for comment-preserving segment body codegen
- All phase-level gaps RESOLVED through phases 1-13
- Remaining 76 snapshot diffs (post-Phase 14 Plan 04):
  - ACCEPTED (aesthetic): SHORTHAND ~8 tests (OXC limitation), LINE_WRAP ~21 tests
  - IMPORT_ORDER: ~36 tests (remaining have other category diffs too)
  - SPREAD_PROPS: RESOLVED (4 tests now exact matches, remaining spread diffs are OXC comment formatting)
  - ENTRY_FIELD: RESOLVED (all 4 tests now exact matches)
  - DEV_MODE: path diffs resolved, remaining diffs are JSX_FLAGS/HOIST
  - FILE_EXT: RESOLVED (preserve_filenames extension fix)
  - CTX_KIND/DIAGNOSTIC: RESOLVED (JSXProp variant, C03 highlights, C05 emission -- 1 new exact match)
  - SIGNAL_WRAP: 11 tests (Phase 15)
  - JSX_FLAGS: 17 tests (Phase 15)
  - DCE: 19 tests (Phase 16)
  - CAPTURES: ~10 tests (Phase 16)
  - HOIST: 14 tests (Phase 17)
  - JSX_IMPORT: 7 tests (Phase 18)
  - OTHER: 13 tests (Phase 18)
  See: .planning/phases/14-final-parity/diff-audit.md for full per-test breakdown

## Session Continuity

Last session: 2026-02-24
Stopped at: Completed 14-04-PLAN.md (Phase 14 complete)
Resume file: None
