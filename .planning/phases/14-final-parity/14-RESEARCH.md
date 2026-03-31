# Phase 14: Cosmetic & Small Fixes - Research

**Researched:** 2026-02-24
**Domain:** Rust/OXC optimizer parity fixes -- import ordering, spread props, metadata, diagnostics
**Confidence:** HIGH

## Summary

Phase 14 targets 7 categories of remaining snapshot diffs that are individually small and low-risk, covering import ordering in entry modules, spread props handling, segment metadata fields, dev mode file paths, file extensions, ctxKind classification, and diagnostic highlights. Together these categories affect ~68 test occurrences (many tests have multiple categories) and fixing them all should yield ~28 new exact matches (bringing the total from 72 to ~100/162).

The research examined every SWC golden snapshot diff for each category, traced the root causes in the OXC codebase, and identified specific code changes needed. Most fixes are 1-20 line changes in well-understood code paths. The import ordering fix is the largest and most impactful (44 tests), requiring a restructuring of the entry module import assembly order.

**Primary recommendation:** Fix in order of impact: (1) import ordering, (2) spread props, (3) entry field, (4) dev mode/file extension/ctxKind/diagnostics as a batch.

## Architecture Patterns

### Category 1: IMPORT_ORDER (44 tests)

**Root cause:** Entry module import assembly in `exit_program` (transform.rs lines 3745-3943) groups and sorts imports incorrectly.

**SWC behavior (from golden snapshots):** SWC uses `ensure_import` with a `BTreeMap<Id>` where `Id = (JsWord, SyntaxContext)`. The `Ord` implementation on `Id` compares `SyntaxContext` (a u32) FIRST, then `JsWord`. Since `SyntaxContext` values are assigned during compilation in encounter order, the effective sort order follows processing/encounter order, NOT alphabetical by name.

**Current OXC behavior:** Splits synthetic imports into two groups (Qrl-suffixed first, framework second), sorts each group alphabetically, then chains them. This produces a different order than SWC.

**What SWC actually does (traced from snapshots):**
1. All `ensure_import` calls go into a single BTreeMap
2. The BTreeMap iterates by (SyntaxContext, JsWord)
3. SyntaxContext values increase in encounter order during traversal
4. Result: imports appear in the order they were first encountered during the fold traversal

**Key observations from snapshot analysis:**
- SWC entry module order: `componentQrl` -> `_wrapProp` -> `_jsxSorted` -> `inlinedQrl` -> `_fnSignal` -> (hoisted _hf stmts) -> `_Fragment` -> `useStore/mutable` -> user imports
- `_Fragment` (from `@qwik.dev/core/jsx-runtime`) is NOT sorted with other framework imports; it comes AFTER hoisted stmts
- Qrl-suffixed imports do NOT always come first; they're mixed with framework imports by encounter order
- OXC puts `_Fragment` in the alphabetically-sorted framework group (wrong position)
- Lazy imports (`const i_XXX = ...`) go between hoisted stmts and `_Fragment` in SWC

**Fix approach:** Instead of alphabetical sorting, maintain the imports in the order they're added to `synthetic_imports` (which follows the code's if-chain order). The order of the if-chain in OXC's Phase 3 (lines 3618-3725) should be adjusted to match SWC's encounter order. Then DON'T sort -- just emit in insertion order. The key ordering groups are:
1. Qrl-suffixed imports (componentQrl, etc.) - encounter order
2. qrl/qrlDEV, inlinedQrl/inlinedQrlDEV - encounter order
3. _captures, _restProps
4. _jsxSorted, _jsx
5. _getVarProps, _getConstProps, _jsxSplit
6. _wrapProp, _fnSignal, _val, _chk
7. _noopQrl/_noopQrlDEV, _qrlSync
8. (hoisted _hf stmts - currently placed in Phase 7 assembly)
9. Lazy imports (const i_XXX)
10. _Fragment (from jsx-runtime) - MUST come AFTER hoisted stmts
11. Non-dollar Qwik core specifiers (useStore, etc.)
12. Non-Qwik user imports
13. Non-import code

**Confidence:** MEDIUM -- The exact SWC encounter order depends on traversal order which may vary per test. The snapshot analysis covers the main patterns but edge cases may require iteration.

### Category 2: SPREAD_PROPS (11 tests)

**Root cause:** OXC always uses `_jsxSplit` + `_getVarProps`/`_getConstProps` for JSX elements with spreads. SWC uses `_createElement` for simple cases.

**SWC behavior:** Two distinct patterns:
1. **Simple spread** (spread-only, no additional explicit props beyond key): Uses `_createElement(tag, { ...source, key })` with all props spread into a single object
2. **Complex spread** (spread + explicit props): Uses `_jsxSplit(tag, { ..._getVarProps(source), explicit }, _getConstProps(source), children, flags, key)` -- `_getConstProps` goes INSIDE the var_props object as a spread, not as a separate argument

**Current OXC diffs (from snapshot analysis):**

a. **`_getConstProps` as separate arg vs spread inside var_props:** For single-spread cases, SWC puts `_getConstProps(source)` as a separate 3rd argument to `_jsxSplit`. OXC does the same. But for the var_props object, SWC does NOT include `..._getConstProps(source)` inside it -- it's a separate argument. OXC currently sometimes puts `..._getConstProps(source)` INSIDE the var_props object (wrong).

b. **`_createElement` pattern missing:** SWC uses `import { createElement as _createElement } from "@qwik.dev/core"` for JSX elements where the only props come from a spread (e.g., `<link {...l} key={l.key} />`). OXC doesn't implement `_createElement` at all, always using `_jsxSplit` instead.

c. **Prop ordering in `_jsxSplit`:** When `_getConstProps` is a separate argument (single spread), explicit props between the spread and later props should be ordered correctly in the var_props object.

**Specific diffs traced:**
- `should_merge_attributes_with_spread_props`: `_getConstProps` should be separate arg, not spread inside var_props
- `should_merge_attributes_with_spread_props_before_and_after`: Uses `...props` directly instead of `..._getVarProps(props)`, and has `_getConstProps(props)` as separate arg -- this is the `_createElement` pattern candidate
- `should_move_bind_value_to_var_props`: `_getConstProps` wrongly spread inside var_props; bind:value placed wrong
- `should_split_spread_props_with_additional_prop`: `_getConstProps` should be separate arg
- `example_spread_jsx`: Needs `_createElement` pattern for simple spread-only elements

**Fix approach:**
1. Implement `_createElement` pattern for simple spread-only elements
2. Fix `_getConstProps` positioning for single-spread `_jsxSplit` calls
3. Fix bind:value placement with spreads

**Confidence:** HIGH -- Diffs are clear and patterns are well-understood from SWC source.

### Category 3: ENTRY_FIELD (4 tests)

**Root cause:** `compute_entry_field` in lib.rs returns `None` for `EntryStrategy::Segment` and passes `&[]` for stack_ctxt in `EntryStrategy::Smart`.

**SWC golden behavior:**
- `example_11` (Segment strategy): Shows `"entry": "entry_segments"` -- SWC returns non-None
- `example_default_export`, `example_manual_chunks`, `example_use_server_mount` (Smart strategy): Show `"entry": "test.tsx_entry_Parent"` etc. -- needs stack_ctxt

**Fix approach:**
1. Change `compute_entry_field` to return `Some("entry_segments")` for `Segment` strategy (matching golden output)
2. Thread `stack_ctxt` (the segment stack context names) through to `segment_data_to_analysis` so Smart/Component strategies can compute correct entry names
3. Store `stack_ctxt` on `SegmentData` during segment creation

**Confidence:** HIGH -- Diffs are simple and the fix is straightforward. The `stack_ctxt` is already available during segment creation (it's `self.segment_stack` + display name components).

### Category 4: DEV_MODE (4 tests)

**Root cause:** OXC test default `src_dir` is `"."` while SWC test default is `"/user/qwik/src/"`. The `dev_abs_path()` function joins `src_dir + "/" + filename`, producing `"./test.tsx"` (OXC) vs `"/user/qwik/src/test.tsx"` (SWC).

**Fix approach:** Change OXC test default `src_dir` from `"."` to `"/user/qwik/src/"` to match SWC's test infrastructure defaults. This is a test config change, not a code logic change. The `dev_abs_path()` function is already correct.

**Confidence:** HIGH -- Root cause is definitively identified as test config mismatch.

### Category 5: FILE_EXT (2 tests)

**Root cause:** When `preserve_filenames = true`, the main module output path should keep the original extension, but OXC applies `output_extension()` unconditionally.

**Tests affected:**
- `example_preserve_filenames`: `transpile_jsx=true`, output should be `test.tsx` not `test.ts`
- `example_preserve_filenames_segments`: `transpile_ts=true, transpile_jsx=true`, output should be `test.tsx` not `test.js`

**Fix approach:** When `preserve_filenames = true`, skip the extension transformation for the main module output path. Change the condition at lib.rs line 222 from:
```rust
if transform_options.transpile_ts || transform_options.transpile_jsx {
```
to:
```rust
if (transform_options.transpile_ts || transform_options.transpile_jsx) && !transform_options.preserve_filenames {
```

**Confidence:** HIGH -- The fix is a single condition change.

### Category 6: CTX_KIND (1 test)

**Root cause:** OXC's `CtxKind` enum has only `Function` and `EventHandler`. SWC has three variants: `Function`, `EventHandler`, and `JSXProp`. JSX attribute values that are function expressions (arrow/function) get `JSXProp`, not `EventHandler`.

**Test affected:** `example_immutable_analysis` -- 3 segments with `"ctxKind": "jSXProp"` in SWC vs `"eventHandler"` in OXC.

**SWC classification logic:**
- `handle_jsx_value` (line 937): event attribute + fn expression -> `EventHandler`; non-event attribute + fn expression -> `JSXProp`
- `internal_handle_jsx_props_obj` (line 1714): fn expression -> `JSXProp`; non-fn -> `EventHandler`
- `handle_qsegment` (line 400): `ctx_name.starts_with("on")` -> `JSXProp`; else -> `Function`

**Fix approach:**
1. Add `JSXProp` variant to `CtxKind` enum with `#[serde(rename = "jSXProp")]`
2. Update `classify_ctx_kind` to return `JSXProp` for appropriate cases
3. Update JSX prop segment creation (transform.rs line 1055) to use `JSXProp` instead of `EventHandler`

**Confidence:** HIGH -- SWC source clearly defines the three variants and their usage.

### Category 7: DIAGNOSTIC (2 tests)

**Root cause:** Two issues:
1. C02/C03 diagnostics don't populate `highlights` field (always `None`)
2. C05 diagnostic (MissingQrlImplementation) not emitted at all

**Tests affected:**
- `example_invalid_segment_expr1`: C03 diagnostics missing highlight spans
- `example_missing_custom_inlined_functions`: C05 diagnostic not emitted; import order diff

**Fix approach for highlights:**
- When emitting C02/C03 diagnostics, compute `SourceLocation` from the identifier's span
- Use the source code to compute line/column from byte offset (similar to `compute_jsx_dev_location` in jsx_transform.rs)
- Add +1 offset to lo/hi for SWC BytePos parity (already done for QRL dev metadata)

**Fix approach for C05:**
- Add detection for non-core $-suffixed function calls where the corresponding Qrl function is not found as a module export
- Emit C05 diagnostic with highlight span pointing to the $-suffixed callee
- This requires checking if `dollar_to_qrl_name(callee)` exists in `collected.module_level_decls` or `collected.exported_local_names`

**Confidence:** HIGH for highlights, MEDIUM for C05 (need to verify exact trigger conditions match SWC).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Source location from byte offset | Manual line counting | Reuse `compute_jsx_dev_location` pattern from jsx_transform.rs | Already proven correct for dev mode |
| Import ordering | Complex sort algorithm | Simple insertion-order preservation | SWC uses encounter order, not sort order |

## Common Pitfalls

### Pitfall 1: Import Order Depends on Encounter Order
**What goes wrong:** Assuming alphabetical sorting matches SWC
**Why it happens:** SWC's BTreeMap<Id> sorts by SyntaxContext (encounter order), not by name
**How to avoid:** Match the if-chain order in exit_program to SWC's traversal encounter order, don't sort
**Warning signs:** Import positions vary between tests in non-alphabetical patterns

### Pitfall 2: _Fragment Position
**What goes wrong:** _Fragment gets sorted with other framework imports
**Why it happens:** _Fragment comes from a different source (`@qwik.dev/core/jsx-runtime`) and SWC adds it to `extra_top_items`, not `ensure_import`
**How to avoid:** Emit _Fragment AFTER hoisted stmts and lazy imports, not with sorted framework imports
**Warning signs:** _Fragment appearing before lazy imports in output

### Pitfall 3: preserve_filenames Affects Extension
**What goes wrong:** Main module path gets extension-transformed even with preserve_filenames
**Why it happens:** Extension transformation is gated only on transpile_ts/transpile_jsx, not preserve_filenames
**How to avoid:** Add preserve_filenames check to extension transformation condition

### Pitfall 4: CtxKind Serialization
**What goes wrong:** `JSXProp` serializes as `jSXProp` (camelCase) not `jsxProp`
**Why it happens:** serde `rename_all = "camelCase"` produces this specific casing
**How to avoid:** Verify with `#[serde(rename = "jSXProp")]` or test the serialization

### Pitfall 5: _getConstProps Placement
**What goes wrong:** _getConstProps spread inside var_props object instead of as separate argument
**Why it happens:** Multi-spread and single-spread have different _getConstProps handling
**How to avoid:** For single-spread _jsxSplit, _getConstProps is ALWAYS a separate 3rd argument, never spread inside var_props

### Pitfall 6: Snapshot Files Must Not Be Committed
**What goes wrong:** Pre-commit hook blocks snapshot file commits
**Why it happens:** Snapshot .snap files are SWC golden references
**How to avoid:** Always run `git checkout -- crates/qwik-optimizer-oxc/tests/snapshots/` after `cargo insta test --accept`

## Code Examples

### Import Order Fix Pattern (transform.rs exit_program)

Instead of sorting, preserve insertion order matching SWC encounter order:
```rust
// Current (wrong): alphabetical sort within groups
qrl_group.sort_by(|a, b| (a.1).0.cmp((b.1).0));
framework_group.sort_by(|a, b| (a.1).0.cmp((b.1).0));

// Fix: don't sort, emit in if-chain order which matches SWC encounter order
// The if-chain in Phase 3 already adds imports in a specific order.
// Just remove the sort and adjust the if-chain order to match SWC.
```

### CtxKind JSXProp Addition (types.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CtxKind {
    #[serde(rename = "eventHandler")]
    EventHandler,
    #[serde(rename = "function")]
    Function,
    #[serde(rename = "jSXProp")]
    JSXProp,
}
```

### preserve_filenames Extension Fix (lib.rs)

```rust
// Current:
let main_path = if transform_options.transpile_ts || transform_options.transpile_jsx {
    // ... apply extension transformation
};

// Fix:
let main_path = if (transform_options.transpile_ts || transform_options.transpile_jsx)
    && !transform_options.preserve_filenames {
    // ... apply extension transformation
};
```

### Entry Field Fix (lib.rs)

```rust
// Current:
EntryStrategy::Segment | EntryStrategy::Hook => None,

// Fix - match golden snapshots:
EntryStrategy::Segment | EntryStrategy::Hook => Some("entry_segments".to_string()),
```

Plus thread stack_ctxt through SegmentData for Smart/Component:
```rust
// In SegmentData, add:
pub stack_ctxt: Vec<String>,

// In segment_data_to_analysis, use it:
let entry = compute_entry_field(entry_strategy, &normalized_origin, &seg.stack_ctxt);
```

### Diagnostic Highlights (transform.rs)

```rust
// Add helper function:
fn compute_source_location(source: &str, lo: u32, hi: u32) -> SourceLocation {
    let (start_line, start_col) = byte_offset_to_line_col(source, lo as usize);
    let (end_line, end_col) = byte_offset_to_line_col(source, hi as usize);
    SourceLocation {
        lo: lo + 1,  // SWC BytePos is 1-based
        hi: hi + 1,
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

// Use in C02/C03 diagnostic emission:
highlights: Some(vec![compute_source_location(&self.source_code, span.start, span.end)]),
```

## Tests That Should Become Exact Matches

After all Phase 14 fixes, these tests should become exact matches (they have ONLY Phase 14 categories):

| Test | Categories | Fix Required |
|------|-----------|-------------|
| example_11 | ENTRY_FIELD | entry field |
| example_default_export | ENTRY_FIELD | entry field |
| example_derived_signals_children | IMPORT_ORDER | import order |
| example_derived_signals_multiple_children | IMPORT_ORDER | import order |
| example_issue_4438 | IMPORT_ORDER | import order |
| example_jsx | IMPORT_ORDER | import order |
| example_manual_chunks | ENTRY_FIELD | entry field |
| example_missing_custom_inlined_functions | IMPORT_ORDER, DIAGNOSTIC | import order + C05 diagnostic |
| example_use_server_mount | ENTRY_FIELD | entry field |
| issue_7216_add_test | SPREAD_PROPS, IMPORT_ORDER | spread props + import order |
| should_merge_attributes_with_spread_props | SPREAD_PROPS | spread props |
| should_merge_attributes_with_spread_props_before_and_after | SPREAD_PROPS | spread props |
| should_move_bind_value_to_var_props | SPREAD_PROPS | spread props |
| should_split_spread_props_with_additional_prop | SPREAD_PROPS | spread props |
| ternary_prop | IMPORT_ORDER | lazy import position |

That's **15 tests** that should become exact matches, plus **~13 more** tests where Phase 14 fixes clear one or more of their diff categories (making them closer to matching, ready for later phases).

## Open Questions

1. **SWC import encounter order accuracy:** The exact encounter order depends on SWC's fold traversal order. The if-chain reordering approach may need fine-tuning per test. An alternative is to record SWC's exact order from golden snapshots and hardcode a priority table.

2. **_createElement threshold:** When does SWC use `_createElement` vs `_jsxSplit` for spread elements? The threshold appears to be "spread-only with no additional explicit props except key", but edge cases may exist. Need to verify against all 11 SPREAD_PROPS test diffs.

3. **C05 diagnostic trigger:** The exact conditions for C05 emission need verification. SWC checks if the Qrl-suffixed function is exported in the same file. OXC's `collected.module_level_decls` and `collected.exported_local_names` should suffice, but the check placement (in which handler) needs to match SWC's `handle_qsegment`.

4. **Entry field for Segment strategy:** The golden snapshots show `"entry_segments"` for `Segment` strategy, but SWC's `PerSegmentStrategy` code returns `None`. Either the golden was generated with an older SWC version, or there's an intermediate layer. Regardless, we match the golden.

## Sources

### Primary (HIGH confidence)
- SWC source code: `crates/swc-optimizer/core/src/transform.rs` -- traced all 7 categories
- SWC source code: `crates/swc-optimizer/core/src/entry_strategy.rs` -- entry policy logic
- SWC source code: `crates/swc-optimizer/core/src/errors.rs` -- diagnostic codes
- SWC test source: `crates/swc-optimizer/core/src/test.rs` -- test defaults (src_dir, entry_strategy)
- Golden snapshots: `crates/qwik-optimizer-oxc/tests/snapshots/*.snap` -- reference output
- OXC current output: `crates/qwik-optimizer-oxc/tests/snapshots/*.snap.new` -- diff targets

### Secondary (MEDIUM confidence)
- Diff audit: `.planning/phases/14-final-parity/diff-audit.md` -- category classification
- STATE.md accumulated decisions -- prior phase context

## Metadata

**Confidence breakdown:**
- Import ordering: MEDIUM -- complex, encounter order may vary
- Spread props: HIGH -- clear patterns from diffs
- Entry field: HIGH -- simple fix, golden output is definitive
- Dev mode: HIGH -- test config difference confirmed
- File extension: HIGH -- single condition change
- CtxKind: HIGH -- SWC source clearly defines 3 variants
- Diagnostics: MEDIUM -- C05 trigger conditions need verification

**Research date:** 2026-02-24
**Valid until:** Indefinite (golden snapshots are static reference)
