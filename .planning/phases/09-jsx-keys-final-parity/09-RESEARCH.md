# Phase 9: JSX Keys & Final Parity - Research

**Researched:** 2026-02-21
**Domain:** OXC optimizer snapshot parity - final closure
**Confidence:** HIGH (all findings from direct codebase analysis and golden snapshot diffing)

## Summary

Phase 9 is the final parity phase. Starting from 135 files diffing with ~4117 diff lines, this phase must close all remaining gaps to achieve 0/162 diffs against SWC golden snapshots. Research reveals the remaining diffs fall into ~18 distinct categories, with the largest being import ordering (79 files), const assignment for bare QRL expressions (39 files), _hf numbering/missing (29+12 files), _fnSignal missing wrapping (12 files), and capture list differences (23 files).

The original Phase 9 success criteria focused on JSX keys and counter ordering, but the actual remaining diff landscape is much broader. The most impactful fixes are: (1) entry module import ordering to match SWC's synthetic import order, (2) preserving const assignment for bare `$()` calls, (3) completing _fnSignal wrapping for props that SWC wraps but OXC doesn't, (4) implementing Hoist strategy function extraction, (5) dev mode qrlDEV/metadata support, (6) computing the `entry` field in segment metadata, (7) fixing className->class transform, and (8) various smaller code generation differences.

**Primary recommendation:** Break this into 4-5 plans organized by dependency: (1) import ordering + const assignment + entry markers (broadest impact, pure mechanical fixes), (2) _fnSignal/hf fixes (missing wrappings and numbering), (3) Hoist strategy extraction + dev mode + code stripping, (4) captures, body code, formatting, and remaining edge cases, (5) final audit.

## Architecture Patterns

### Diff Category Taxonomy (135 files, ~4117 diff lines)

Comprehensive analysis of every remaining diff, categorized by root cause:

| Category | Files | Diff Lines | Root Cause |
|----------|-------|------------|------------|
| **A: Import order only** | 14 | ~70 | OXC emits synthetic imports in hardcoded order; SWC uses traversal encounter order |
| **B: Import set diff** | 23 | ~200 | Missing/extra imports (qrlDEV, _noopQrlDEV, _fnSignal, etc.) |
| **C: Hoist fn extraction** | 11 | ~300 | SWC Hoist extracts callback to named const; OXC keeps inline |
| **D: _hf missing entirely** | 11 | ~150 | OXC not wrapping some expressions with _fnSignal (thus no _hf generated) |
| **E: _hf numbering** | 12 | ~100 | Counter offset when some _hf entries are missing |
| **F: _fnSignal missing** | 12 | ~120 | Props expressions not wrapped with _fnSignal that SWC wraps |
| **G: Key value mismatch** | 8 | ~40 | Counter skews from missing Hoist extraction, or &&-element keys |
| **H: Flag mismatch** | 18 | ~60 | 33 remaining flag mismatches (all from upstream prop/transform diffs) |
| **I: Entry marker** | 8 | ~50 | `"entry": null` when SWC has `"entry": "entry_segments"` |
| **J: Dev mode** | 4 | ~250 | qrlDEV, _noopQrlDEV, dev metadata not implemented |
| **K: _noopQrl missing** | 2 | ~20 | _noopQrl not emitted in non-dev strip scenarios |
| **L: Code stripping** | 3 | ~150 | isServer/isBrowser code not stripped or partially stripped |
| **M: Const assignment** | 39 | ~80 | SWC wraps bare `$()` result in `const X = `; OXC drops it |
| **N: className transform** | 1 | ~15 | `className` not converted to `class` for native HTML elements |
| **O: Capture diffs** | 23 | ~200 | Missing/extra captures, capture formatting (single const vs chained) |
| **P: Body code diff** | 5 | ~30 | Dead code elimination differences (SWC strips more code from segments) |
| **Q: Format JSX attr** | 2 | ~10 | OXC codegen single-line vs SWC multi-line for JSX attributes |
| **R: Text spacing** | 1 | ~5 | "data-nu: " vs "data-nu:" trailing space in JSX text nodes |

**Note:** Many files have multiple overlapping categories. The total unique file count is 135.

### Key Dependency Chain

```
Import Ordering (A) ─── standalone, broadest impact (79 files)
Const Assignment (M) ── standalone, broad impact (39 files)
Entry Markers (I) ───── standalone, needs entry_strategy policy
_fnSignal/hf (D,E,F) ─ must fix F first, then D follows, E renumbers
Hoist Extraction (C) ── depends on understanding segment.expr in SWC
Dev Mode (J) ────────── depends on EmitMode::Dev detection
Captures (O) ────────── may partially resolve with F fixes
Flags (H) ──────────── resolves automatically when upstream diffs fixed
Keys (G) ──────────── resolves when counter ordering from C/D fixed
```

### Recommended Implementation Structure

```
Plan 1: Import ordering + const assignment + entry markers
  - Fix synthetic import ordering in exit_program
  - Preserve const binding for bare $() calls
  - Compute entry field from EntryStrategy

Plan 2: _fnSignal wrapping completeness + _hf numbering
  - Identify which expressions SWC wraps but OXC doesn't
  - Fix _hf counter to match SWC ordering
  - Fix text spacing in JSX text nodes

Plan 3: Hoist extraction + className + body code
  - Implement Hoist strategy named-const extraction
  - Add className->class transform for native elements
  - Fix segment body code (dead code, expression formatting)

Plan 4: Dev mode + code stripping + _noopQrl
  - Implement qrlDEV and _noopQrlDEV with dev metadata
  - Fix isServer/isBrowser stripping
  - Complete _noopQrl for non-dev strip scenarios

Plan 5: Captures + formatting + final audit
  - Fix capture list completeness
  - Fix capture formatting (individual const vs chained)
  - Fix JSX attribute line breaks
  - Fix remaining edge cases
  - Full 162-snapshot audit
```

## Code Examples

### 1. Import Ordering Fix (Category A)

SWC emits synthetic imports in the order they appear in `global_collect.synthetic`, which is populated during fold traversal order. OXC's `exit_program` builds them in a hardcoded order (Phase 3, lines 2705-2800 of transform.rs).

**SWC order for Hoist/Inline tests:**
```
componentQrl, inlinedQrl, _jsxSorted, _wrapProp, _fnSignal, Fragment, useStore/mutable, dep
```

**OXC order (current):**
```
componentQrl, inlinedQrl, _jsxSorted, _wrapProp, _fnSignal, Fragment, useStore/mutable, dep
```

The ordering may differ because OXC builds all synthetic imports up-front in Phase 3, while SWC discovers them during traversal. The specific order depends on which `needs_*` flags get set first.

**Fix approach:** Sort synthetic imports by local name (alphabetical by `Atom`) to match SWC's `create_synthetic_named_import` order from `global_collect.synthetic`. Alternatively, mirror SWC's ordering by emitting them in the same hardcoded sequence as SWC's collector populates them.

**Source:** `crates/qwik-optimizer-oxc/src/transform.rs` lines 2700-2830 (exit_program Phase 3)
**SWC ref:** `crates/swc-optimizer/core/src/transform.rs` lines 2600-2608 (fold_module synthetic emission)

### 2. Const Assignment Fix (Category M, 39 files)

SWC preserves the original variable binding when a `$()` call replaces the callback body. For example:

```js
// Input
const Header = $((ev) => console.log(ev));

// SWC output (segment strategy)
const Header = /* @__PURE__ */ qrl(i_XXX, "Header_XXX");

// OXC output (bug)
/* @__PURE__ */ qrl(i_XXX, "Header_XXX");  // Lost 'const Header = '
```

The issue is that OXC replaces the inner expression (the `$()` call) but loses the surrounding `VariableDeclaration` context. When the `$()` call is the init of a `const X = $(() => ...)`, OXC replaces `$(() => ...)` with the qrl call but the variable declarator's init gets replaced, and somehow the const binding is lost.

**Source:** `crates/qwik-optimizer-oxc/src/transform.rs` exit_expression around line 2568-2648

### 3. Entry Field Fix (Category I, 8 files)

The `entry` field in segment metadata is hardcoded to `None`:

```rust
// crates/qwik-optimizer-oxc/src/lib.rs line 380
entry: None, // Phase 10 will set this
```

SWC computes it from the EntryPolicy:

```rust
// SWC: crates/swc-optimizer/core/src/transform.rs line 827
let entry = self.options.entry_policy.get_entry_for_sym(&self.stack_ctxt, &segment_data);
```

For `Inline`/`Hoist` strategies, `entry = Some("entry_segments")`.
For `Segment` strategy, `entry = None`.

**Fix:** Compute entry based on strategy in `segment_data_to_analysis()`.

### 4. Hoist Strategy Function Extraction (Category C, 11 files)

SWC's `fold_module` for `EntryStrategy::Hoist` drains accumulated segments and creates named const declarations:

```rust
// SWC fold_module (lines 2567-2592):
if matches!(self.options.entry_strategy, EntryStrategy::Hoist) {
    self.segments.drain(..)
        .map(|segment| {
            const Name_hash = segment.expr;  // Named const declaration
        })
        .chain(iter::once(module_item))  // Original statement after
}
```

This produces:
```js
// SWC Hoist output
const AppDynamic1_component_R00UJ05gbes = (props) => { ... };
export const AppDynamic1 = componentQrl(inlinedQrl(
  AppDynamic1_component_R00UJ05gbes, "AppDynamic1_component_R00UJ05gbes"
));
```

OXC currently treats Hoist same as Inline:
```js
// OXC Hoist output (wrong)
export const AppDynamic1 = componentQrl(inlinedQrl(
  (props) => { ... }, "AppDynamic1_component_R00UJ05gbes"
));
```

**Fix:** In exit_program or a post-traversal step, for Hoist strategy, extract the first argument of each `inlinedQrl()` call into a preceding named const declaration using the segment name.

### 5. Dev Mode (Category J, 4 files)

SWC emits `qrlDEV` instead of `qrl` when `EmitMode::Dev`, adding dev metadata:
```js
qrlDEV(import_fn, "name", { file: "/path", lo: N, hi: N, displayName: "..." })
```

And `_noopQrlDEV` instead of `_noopQrl`:
```js
_noopQrlDEV("name", { file: "/path", lo: 0, hi: 0, displayName: "..." }, [captures])
```

OXC currently ignores EmitMode::Dev and always emits plain `qrl`/`_noopQrl`.

### 6. Windows Path Hash (Success Criterion 3)

The `support_windows_paths` test shows key "9H_0" (OXC) vs "KD_0" (SWC). The hash difference comes from how the file path is normalized before hashing. SWC normalizes backslashes to forward slashes before computing the hash; OXC preserves backslashes.

Additionally, the `origin` field shows `"components\\apps\\apps.tsx"` (OXC) vs `"components/apps/apps.tsx"` (SWC) and the module path shows `components\apps\apps.ts` vs `components/apps/apps.ts`.

**Fix:** Normalize backslashes to forward slashes in origin paths and hash inputs.

### 7. relative_paths Test (Success Criterion 4)

This test has a completely different output because the `additional_inputs` (the dep file) is being processed differently. SWC treats the dep file as pre-compiled (already containing inlinedQrl calls) and passes it through mostly unchanged. OXC seems to be re-processing it, creating segment extractions where it shouldn't.

The dep code uses `inlinedQrl()` directly -- it's already compiled. SWC should emit it with minimal changes, but OXC appears to be extracting segments from it.

**Fix:** The dep file needs special handling -- when it already contains `inlinedQrl()` calls, they should not be re-processed as $() dollar calls. The OXC optimizer should detect pre-compiled QRL patterns and skip extraction.

## Common Pitfalls

### Pitfall 1: Import Ordering is Insertion-Order Dependent
**What goes wrong:** Changing the order of `needs_*` flag checks changes import order
**Why it happens:** SWC's import order comes from when imports are discovered during fold, not from a sort
**How to avoid:** Either match the exact SWC order or sort alphabetically by local name (SWC's `IdentCollector.get_words()` does sort for segment modules)

### Pitfall 2: _hf Counter is Global Across All Segments
**What goes wrong:** Fixing _fnSignal wrapping for some props changes all _hf numbers
**Why it happens:** _hf counter is monotonic across the entire module
**How to avoid:** Fix all _fnSignal wrapping issues in one pass before auditing _hf numbering

### Pitfall 3: Cascading Diffs from Single Fixes
**What goes wrong:** Fixing one category (e.g., Hoist extraction) changes key counter, flag values, capture lists
**Why it happens:** Key counter, flags, and captures all depend on the code structure produced by transforms
**How to avoid:** Work in dependency order: structural changes first (imports, const assignment, Hoist), then derived values (keys, flags, captures) will often self-correct

### Pitfall 4: Snapshot Hashes Hide Regressions
**What goes wrong:** Hash replacement with XXXXXXXXXXXX masks real hash changes
**Why it happens:** `replace_hashes()` only replaces hashes found in `"hash": "..."` metadata
**How to avoid:** For inline/hoist strategies where no separate metadata is emitted, literal hashes in inlinedQrl names are NOT replaced and must match exactly

### Pitfall 5: relative_paths is a Multi-Input Test
**What goes wrong:** Assuming all tests are single-file transforms
**Why it happens:** relative_paths has `additional_inputs` with a pre-compiled dep file
**How to avoid:** The dep file uses `inlinedQrl()` directly; the optimizer must not re-extract segments from pre-compiled QRL patterns

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Import ordering | Custom sort algorithm | Match SWC's `global_collect.synthetic` emission order | Import order is defined by SWC, not by any standard |
| Entry field computation | Hardcoded values | Port SWC's EntryPolicy trait | Different strategies have different entry rules |
| Hoist extraction | New traversal pass | Post-process in exit_program | All data already available at that point |
| Dev mode metadata | Guessing span values | Port SWC's dev metadata construction | Span values (lo, hi) must match SWC exactly |

## Open Questions

1. **Import ordering precision**
   - What we know: SWC's entry module imports come from `global_collect.synthetic` Vec (insertion order), not sorted
   - What's unclear: The exact insertion order depends on SWC's collector traversal, which differs from OXC's
   - Recommendation: Try alphabetical sort first; if that doesn't match, trace SWC's collector to determine insertion order

2. **relative_paths dep file handling**
   - What we know: The dep file contains pre-compiled inlinedQrl calls that OXC is re-processing
   - What's unclear: Whether the issue is in how additional_inputs are processed or in QRL detection
   - Recommendation: Investigate whether OXC's `$()` detection fires on `inlinedQrl()` calls in the dep file

3. **Capture chained vs individual const**
   - What we know: SWC emits `const a = _captures[0], b = _captures[1];` (single const, chained)
   - OXC emits `const a = _captures[0]; const b = _captures[1];` (individual consts)
   - Recommendation: Modify capture codegen in code_move.rs to use chained declarators

4. **Body code differences (5 files)**
   - What we know: SWC strips some declarations/expressions from segment bodies that OXC preserves
   - What's unclear: The exact dead code elimination rules SWC applies
   - Recommendation: Compare SWC's code_move logic for body expression handling

5. **_fnSignal wrapping for specific prop patterns**
   - What we know: 12 files have _fnSignal wrapping that SWC does but OXC doesn't
   - What's unclear: Which specific expression patterns trigger _fnSignal in SWC but not OXC
   - Recommendation: Diff the specific expressions in those 12 files to identify the gap

## Sources

### Primary (HIGH confidence)
- Direct codebase analysis: `crates/qwik-optimizer-oxc/src/transform.rs`, `crates/qwik-optimizer-oxc/src/lib.rs`
- SWC reference: `crates/swc-optimizer/core/src/transform.rs`, `crates/swc-optimizer/core/src/entry_strategy.rs`
- Golden snapshots: `crates/qwik-optimizer-oxc/tests/snapshots/*.snap`
- git diff analysis of all 135 remaining diff files
- Test configuration: `crates/qwik-optimizer-oxc/tests/test.rs`

## Metadata

**Confidence breakdown:**
- Diff taxonomy: HIGH - based on exhaustive analysis of all 135 files
- Architecture patterns: HIGH - from direct SWC source comparison
- Fix approaches: MEDIUM - some fixes are straightforward, others need investigation during implementation
- Impact estimates: MEDIUM - file counts are exact, but cascading effects may resolve or create new diffs

**Research date:** 2026-02-21
**Valid until:** Until Phase 9 is implemented (diffs will change as fixes are applied)
