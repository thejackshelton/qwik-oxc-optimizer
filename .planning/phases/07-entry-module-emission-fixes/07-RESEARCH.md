# Phase 7: Entry Module Emission Fixes - Research

**Researched:** 2026-02-21
**Domain:** Rust / OXC AST manipulation / Qwik optimizer entry module code generation
**Confidence:** HIGH

## Summary

This phase addresses three distinct bugs in how the OXC optimizer emits entry modules and segment files. All three bugs stem from a fundamental architectural difference between SWC and OXC: SWC stores hoisted declarations (`_hf*`, lazy imports) in a single `BTreeMap<Id>` called `extra_top_items` and relies on DCE (dead code elimination via `simplify::simplifier`) to clean unused items from each output module. OXC doesn't have DCE, so it must explicitly filter what goes where.

The three bugs are:
1. **GAP-1 (Critical):** `_hf*` declarations leak into the entry module because `lib.rs` lines 168-199 inject `hoisted_function_stmts` unconditionally via string manipulation after AST emission, bypassing the `collect_referenced_idents` filter in `exit_program`.
2. **GAP-7 (Minor):** `_fnSignal` import falsely added to segment files because `code_move.rs` line 106 checks `!hoisted_stmts.is_empty()` as a proxy for `_fnSignal` usage, but hoisted stmts existing globally doesn't mean this segment uses `_fnSignal`.
3. **GAP-2 (Significant, partially fixed):** Lazy import ordering was sorted by hash (fixed in `d584993`), but the entry module may still have ordering issues due to `_hf*` leakage and extra imports distorting the diff count.

**Primary recommendation:** Fix GAP-1 by skipping `hoisted_stmts` injection in `lib.rs` for segment strategy (the segment files in `code_move.rs` already handle `_hf*`). Fix GAP-7 by removing the `|| !hoisted_stmts.is_empty()` check. Add per-segment filtering of `hoisted_stmts` in `code_move.rs` to only inject the `_hf*` declarations each segment actually references.

## Standard Stack

No new libraries needed. All changes are in existing Rust source files.

### Core Files to Modify

| File | Lines | Purpose | Change Type |
|------|-------|---------|-------------|
| `crates/qwik-optimizer-oxc/src/lib.rs` | 167-199 | Entry module `_hf*` injection | Conditional skip for segment strategy |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | 106 | `_fnSignal` import false-positive | Remove `\|\| !hoisted_stmts.is_empty()` |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | 269-273 | Per-segment `_hf*` injection | Filter by body_code reference |

## Architecture Patterns

### How SWC Handles Hoisted Declarations

SWC stores ALL hoisted items (lazy imports `i_{hash}`, `_hf*` declarations, `_hf*_str` declarations) in a single `BTreeMap<Id, ModuleItem>` called `extra_top_items` (transform.rs line 103). This map is:

1. **Populated during transform:** `self.extra_top_items.insert(fn_id, const_decl)` at line 2139 for `_hf*`, line 1285 for lazy imports
2. **Injected into entry module:** `body.extend(self.extra_top_items.values().cloned())` in `fold_module` at line 2609
3. **Injected into EVERY segment file:** `module.body.extend(ctx.extra_top_items.values().cloned())` in `code_move.rs` line 147
4. **Cleaned by DCE:** `simplify::simplifier` runs when `minify != MinifyMode::None` (parse.rs line 342-352), removing unused declarations

Since the default test config uses `MinifyMode::Simplify`, DCE runs and removes:
- `_hf*` from entry module (only referenced in segment bodies)
- Unused `_hf*` from segments that don't reference them
- `_fnSignal` import from modules that don't use it

**Confidence: HIGH** -- Verified by reading SWC source: transform.rs lines 103, 2139, 2609; code_move.rs line 147; parse.rs lines 336-352; test.rs line 5213 (`MinifyMode::Simplify`).

### How OXC Currently Handles Hoisted Declarations

OXC uses a fundamentally different approach:

1. `hoisted_function_stmts: Vec<(String, String)>` stores `(fn_code, str_code)` pairs as strings
2. **Entry module injection (lib.rs 167-199):** String manipulation inserts ALL hoisted stmts between last import and first non-import in the emitted code. This happens AFTER `exit_program`, bypassing `collect_referenced_idents` filtering. **Unconditional -- runs for all strategies.**
3. **Segment file injection (code_move.rs 269-273):** ALL hoisted stmts injected into every segment file. No filtering.
4. **No DCE:** OXC doesn't run dead code elimination

**Confidence: HIGH** -- Verified by reading OXC source directly.

### Root Cause Analysis

| Gap | Root Cause | Impact |
|-----|-----------|--------|
| GAP-1 | `lib.rs` injects `hoisted_stmts` unconditionally into entry module | ~50 tests, 172 added lines |
| GAP-7 | `code_move.rs` checks `!hoisted_stmts.is_empty()` globally | ~25 extra `_fnSignal` imports |
| GAP-2 | Lazy import sort by hash is correct, but extra items distort count | ~80 tests (overlap with GAP-1) |
| Bonus | `code_move.rs` injects ALL `_hf*` into every segment, not filtered | Unknown exact count, some diffs |

### Fix Strategy

Instead of implementing DCE (which would be a massive effort and out of scope), apply targeted filtering:

**GAP-1 Fix:** In `lib.rs`, check if the strategy is segment-based. If so, skip the `hoisted_stmts` string injection for the entry module. For inline/hoist strategy, keep the injection since the component body stays in the entry module and references `_hf*`.

```rust
// lib.rs line ~168
let is_inline_like = entry_strategy::should_inline(&transform_options.entry_strategy)
    || matches!(transform_options.entry_strategy, EntryStrategy::Hoist);

let main_code = if !hoisted_stmts.is_empty() && is_inline_like {
    // ... existing string injection code ...
} else {
    emit_result.code.clone()
};
```

**GAP-7 Fix:** In `code_move.rs` line 106, remove the `|| !hoisted_stmts.is_empty()` condition:

```rust
// Before:
if body_code.contains("_fnSignal") || !hoisted_stmts.is_empty() {

// After:
if body_code.contains("_fnSignal") {
```

**Per-segment `_hf*` filtering:** In `code_move.rs` lines 269-273, only inject hoisted stmts whose variable name appears in the segment's body_code:

```rust
// Before:
for (fn_code, str_code) in hoisted_stmts {
    parts.push(fn_code.clone());
    parts.push(str_code.clone());
}

// After:
for (fn_code, str_code) in hoisted_stmts {
    // Extract the variable name from "const _hfN = ..."
    // and check if the segment body references it
    if let Some(var_name) = fn_code.strip_prefix("const ").and_then(|s| s.split('=').next()).map(|s| s.trim()) {
        if body_code.contains(var_name) {
            parts.push(fn_code.clone());
            parts.push(str_code.clone());
        }
    } else {
        // Fallback: include if we can't parse the name
        parts.push(fn_code.clone());
        parts.push(str_code.clone());
    }
}
```

### Anti-Patterns to Avoid

- **Don't implement full DCE:** That's a massive effort. Targeted filtering achieves the same result for these specific cases.
- **Don't modify `exit_program` for `_hf*`:** The `collect_referenced_idents` approach won't work because `_hf*` are string-injected AFTER AST emission. The fix must be at the injection point (`lib.rs`).
- **Don't assume `_hf*` naming pattern is unstable:** The `const _hfN = ...` pattern is stable -- it's generated by `hoist_fn_signal_call` in both SWC and OXC with monotonic counter.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Dead code elimination | Full DCE pass | Targeted body_code.contains() filtering | DCE is complex; the specific patterns we need to filter are known and simple |
| AST-level `_hf*` tracking | AST node injection for hoisted stmts | String-level filtering at injection point | The existing string-based approach works; just add filtering |

## Common Pitfalls

### Pitfall 1: Inline Strategy Regression
**What goes wrong:** Removing `_hf*` injection from entry module breaks inline/hoist strategy where the component body stays in the entry module
**Why it happens:** The fix must be conditional on strategy type
**How to avoid:** Check `is_inline_like` before skipping injection
**Warning signs:** `example_inlined_entry_strategy` or `example_parsed_inlined_qrls` tests failing

### Pitfall 2: `_hf*` Variable Name Extraction Fragility
**What goes wrong:** The per-segment filtering relies on extracting variable names from `const _hfN = ...` strings
**Why it happens:** String parsing can break if the format changes
**How to avoid:** The format is stable (generated by `hoist_fn_signal_call`), but use a robust extraction method. Consider storing the variable name alongside the code strings.
**Better approach:** Add the variable name to the `hoisted_function_stmts` tuple: `Vec<(String, String, String)>` where the third element is the base name like `_hf0`. This avoids string parsing entirely.

### Pitfall 3: Ordering of `_hf*` in Segment Files vs SWC
**What goes wrong:** Even after filtering, the `_hf*` declarations in segment files might be in wrong order
**Why it happens:** SWC uses `BTreeMap<Id>` which sorts `_hf0`, `_hf0_str`, `_hf1`, `_hf1_str` alphabetically. OXC iterates `hoisted_function_stmts` in push order.
**How to avoid:** The push order in OXC already follows alphabetical order (monotonic counter), so `_hf0` comes before `_hf1`. The `(fn_code, str_code)` pairs are emitted fn then str, which matches `_hf0` < `_hf0_str` in BTreeMap ordering (since `_` sorts before `_` + char in same-prefix comparison, actually `_hf0` < `_hf0_str` because `_hf0` is shorter).

Actually, in `BTreeMap<Id>` ordering: `_hf0` vs `_hf0_str` -- `Atom` comparison is string comparison. `"_hf0"` < `"_hf0_str"` because at position 4, `'\0' (end) < '_'`. So SWC order is: `_hf0`, `_hf0_str`, `_hf1`, `_hf1_str`. OXC's push order emits `(fn_code, str_code)` per pair, which is `_hf0`, `_hf0_str`, `_hf1`, `_hf1_str`. This matches.

**Confidence: HIGH** -- String comparison ordering verified.

### Pitfall 4: Entry Module `_fnSignal` Import in Segment Strategy
**What goes wrong:** After fixing GAP-1, the `_fnSignal` import might still appear in the entry module
**Why it happens:** `needs_fn_signal` is set on the import_tracker during transform, and `exit_program` builds a synthetic import for it. But `referenced_idents` filtering in `exit_program` should already handle this IF `_hf*` declarations are not in the entry module body.
**How to avoid:** After fixing GAP-1 (removing `_hf*` from entry module), the `_fnSignal` references are only in segment bodies, so `collect_referenced_idents` won't find `_fnSignal` and the synthetic import will be filtered out. This should work automatically.
**Warning signs:** If `_fnSignal` still appears in entry modules after GAP-1 fix, check if `_fnSignal` calls are in non-extracted code (e.g., inside inline QRL bodies that stay in the entry module).

### Pitfall 5: `is_inline_like` Check Placement
**What goes wrong:** The variable `is_inline_like` is already computed at line 237-238 of `lib.rs`, but it's computed AFTER the `main_code` block (lines 167-199). Need to either move the computation earlier or compute it independently.
**How to avoid:** Compute `is_inline_like` before the `main_code` block, or use the `transform_options.entry_strategy` directly in the condition.

## Code Examples

### GAP-1 Fix: Conditional `_hf*` Injection in lib.rs

```rust
// lib.rs, around line 167
// Determine strategy before the hoisted_stmts injection
let is_inline_like_strategy = entry_strategy::should_inline(&transform_options.entry_strategy)
    || matches!(transform_options.entry_strategy, EntryStrategy::Hoist);

let hoisted_stmts: Vec<(String, String)> = qwik_transform.hoisted_function_stmts().to_vec();
let main_code = if !hoisted_stmts.is_empty() && is_inline_like_strategy {
    // For inline/hoist strategy: inject _hf* into entry module
    // (component body stays in entry module and references them)
    let mut hoisted_code = String::new();
    for (fn_code, str_code) in &hoisted_stmts {
        hoisted_code.push_str(fn_code);
        hoisted_code.push('\n');
        hoisted_code.push_str(str_code);
        hoisted_code.push('\n');
    }
    // ... rest of existing insertion logic ...
} else {
    // For segment strategy: _hf* only go to segment files via code_move.rs
    emit_result.code.clone()
};
```

### GAP-7 Fix: Remove False-Positive Check in code_move.rs

```rust
// code_move.rs, line 106
// Before:
if body_code.contains("_fnSignal") || !hoisted_stmts.is_empty() {
// After:
if body_code.contains("_fnSignal") {
    imports.push(SegmentImportEntry {
        local_name: "_fnSignal".to_string(),
        source: core.clone(),
        kind: ImportKind::Named,
        imported_name: None,
    });
}
```

### Per-Segment `_hf*` Filtering in code_move.rs

```rust
// code_move.rs, lines 269-273
// Before: inject ALL hoisted stmts into every segment
for (fn_code, str_code) in hoisted_stmts {
    parts.push(fn_code.clone());
    parts.push(str_code.clone());
}

// After: only inject hoisted stmts that the segment body actually references
for (fn_code, str_code) in hoisted_stmts {
    // Extract variable name: "const _hf0 = ..." -> "_hf0"
    if let Some(var_name) = fn_code
        .strip_prefix("const ")
        .and_then(|s| s.split(|c: char| c == ' ' || c == '=').next())
    {
        if body_code.contains(var_name) {
            parts.push(fn_code.clone());
            parts.push(str_code.clone());
        }
    } else {
        parts.push(fn_code.clone());
        parts.push(str_code.clone());
    }
}
```

### Alternative: Store Variable Name in Tuple (Recommended)

Instead of parsing strings, modify the `hoisted_function_stmts` type:

```rust
// In transform.rs, change:
hoisted_function_stmts: Vec<(String, String)>,
// To:
hoisted_function_stmts: Vec<(String, String, String)>,
// Where tuple is: (fn_declaration_code, str_declaration_code, base_var_name)
// e.g., ("const _hf0 = (p0)=>p0.value;", "const _hf0_str = \"p0.value\";", "_hf0")
```

Then in `code_move.rs`:
```rust
for (fn_code, str_code, var_name) in hoisted_stmts {
    if body_code.contains(var_name.as_str()) {
        parts.push(fn_code.clone());
        parts.push(str_code.clone());
    }
}
```

This is more robust but requires updating all call sites. The string parsing approach is acceptable given the stable `const _hfN` format.

## State of the Art

| Old Approach (Current OXC) | New Approach (This Phase) | Impact |
|---------------------------|--------------------------|--------|
| Inject ALL `_hf*` into entry module unconditionally | Skip injection for segment strategy | Removes ~172 diff lines across ~50 tests |
| `_fnSignal` import added when ANY hoisted stmt exists | Import only when segment body contains `_fnSignal` | Removes ~25 false-positive imports |
| ALL `_hf*` injected into every segment | Per-segment filtering by body_code reference | Removes extra `_hf*` from segments that don't use them |

## Open Questions

1. **Exact diff count reduction:** The milestone audit estimates 138->80 (58 fewer diffs), but the actual reduction depends on how many of the 138 diffs are SOLELY caused by these three issues vs. overlapping with other issues (JSX flags, capture lists, etc.). The 172 `_hf*` added lines span ~50 tests, and the import ordering (~80 tests) overlaps significantly. Realistically, fixing GAP-1 + GAP-7 should fix tests where `_hf*` leakage was the ONLY remaining diff, but tests with multiple issues may not become exact matches.

2. **Inline strategy `_hf*` ordering:** For inline strategy, `_hf*` declarations in the entry module need to be in `BTreeMap<Id>` order relative to lazy imports. Currently they're injected separately (lazy imports by AST, `_hf*` by string). This may cause ordering mismatches for inline strategy tests, but those are a small subset.

3. **`_hf*` in SWC segment files after DCE:** SWC injects ALL `extra_top_items` then DCE removes unused ones. For a segment that uses `_hf0` but not `_hf1`, SWC DCE removes `_hf1`. The OXC per-segment filtering must match this behavior. The `body_code.contains("_hf0")` check should work since `_fnSignal(_hf0, ...)` literally contains `_hf0`.

## Sources

### Primary (HIGH confidence)
- SWC transform.rs lines 103, 2109-2168, 2600-2614: `extra_top_items BTreeMap<Id>`, `_hf*` insertion, `fold_module` assembly
- SWC code_move.rs line 147: `extra_top_items` injected into segment files
- SWC parse.rs lines 336-352: DCE runs when `minify != MinifyMode::None`
- SWC test.rs line 5213: Default `MinifyMode::Simplify` (DCE enabled in tests)
- OXC lib.rs lines 167-199: Unconditional `hoisted_stmts` string injection
- OXC code_move.rs line 106: `_fnSignal` false-positive check
- OXC code_move.rs lines 269-273: Unfiltered `_hf*` injection
- OXC transform.rs line 2683: Lazy import sort by hash (already fixed)
- SWC snapshot `example_getter_generation`: Confirms entry module has NO `_hf*`, segments have only their own `_hf*`

### Secondary (MEDIUM confidence)
- Phase 6 VERIFICATION.md: Confirms 138 diffs remain, lazy import sort key was partial fix
- STATE.md: Confirmed `MinifyMode::Simplify` default, DCE dependency for SWC

## Metadata

**Confidence breakdown:**
- GAP-1 analysis: HIGH - SWC source code and snapshot comparison directly confirm the issue
- GAP-7 analysis: HIGH - Code path is trivial, diff clearly shows false-positive `_fnSignal` imports
- GAP-2 analysis: HIGH - Sort key was already fixed in `d584993`; remaining ordering issues likely from GAP-1 interactions
- Per-segment `_hf*` filtering: HIGH - SWC snapshot `example_getter_generation` directly shows different `_hf*` sets per segment
- Fix approach: HIGH - Targeted filtering at injection points, no complex architectural changes

**Research date:** 2026-02-21
**Valid until:** Stable (these are codebase-specific patterns, not external dependencies)
