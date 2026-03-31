# Phase 6: Import Ordering & Cleanup - Research

**Researched:** 2026-02-20
**Domain:** AST import manipulation, dead code elimination, entry/segment module scoping
**Confidence:** HIGH

## Summary

Phase 6 is the final cleanup phase targeting 0/162 snapshot diffs. After analyzing all 156 remaining snapshot diffs, I found that only **9 snapshots have import-only diffs** -- the other 147 have non-import code-level diffs as well. This means Phase 6 must address both import-specific issues AND the deferred code-level issues from earlier phases.

The import issues fall into three distinct categories:
1. **Extra imports in entry module** (IMP-02): OXC emits ALL tracked imports (framework + user) in the entry module, even when they're only used in segment bodies. SWC uses DCE and scope-aware emission to keep entry modules clean.
2. **Import ordering** (IMP-01): Both entry and segment modules emit imports in the wrong order compared to SWC. SWC uses sorted `local_idents`, insertion-order synthetic imports, and BTreeMap-ordered `extra_top_items`.
3. **Specifier merging** (IMP-03): SWC preserves original multi-specifier imports (e.g., `import { useStore, mutable }`) when they pass through the fold unchanged. OXC re-emits each specifier as a separate import.

The deferred code-level issues (QRL hoisting, non-component props destructuring, flags, etc.) significantly outnumber the pure import issues and must be addressed in this phase.

**Primary recommendation:** Split Phase 6 into three plans: (1) fix entry module scoping (stop emitting segment-only imports), (2) fix import ordering in both entry and segment modules + specifier merging, (3) address remaining code-level diffs (QRL hoisting, destructuring, flags, misc).

## Standard Stack

No new libraries are needed. All work uses existing OXC APIs.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| oxc (parser, codegen, traverse) | current | AST manipulation | Already in use |
| oxc_traverse | current | AST traversal | Already in use |

### Supporting
No additional libraries needed. The key missing piece is not a library but rather **scoping logic** -- OXC needs to track which imports are needed at entry-module level vs segment level.

## Architecture Patterns

### Current Architecture (Problem)

```
Entry Module Import Emission (exit_program):
  1. import_tracker flags -> ALL emitted as entry module imports
  2. Non-dollar Qwik core specifiers -> ALL re-emitted
  3. Old body items -> Non-Qwik imports preserved as-is

Segment Module Import Emission (code_move.rs):
  1. body_code.contains("_jsxSorted") etc -> string-based detection
  2. needed_imports from capture analysis -> individual imports
  3. Hardcoded ordering of framework imports
```

### SWC Architecture (Target)

```
Entry Module:
  1. synthetic imports (Vec insertion order during traversal)
     - ONLY includes ensure_core_import() calls made during
       ENTRY MODULE code processing (not inside $-call bodies)
  2. extra_top_items (BTreeMap<Id> sorted order)
     - Lazy import declarations (const i_hash = ...)
     - Hoisted function declarations (_hf0, _hf0_str)
  3. Original module body (fold transforms in-place)
     - Original non-Qwik imports pass through UNCHANGED
     - Build constants replaced by const_replace
     - DCE (simplifier) removes unused imports
  4. extra_bottom_items (BTreeMap<Id> sorted order)

Segment Module:
  1. _captures import (if needed)
  2. Imports from sorted local_idents:
     - local_idents = HashSet<Id> collected by IdentCollector
     - Sorted: local_idents.sort() (alphabetical by (Atom, SyntaxContext))
     - Each id looked up in global_collect.imports to get source/kind
  3. extra_top_items (BTreeMap sorted)
  4. Export declaration
```

### Required Architecture Changes

#### A. Entry Module Scoping

The core problem: `import_tracker` flags are set globally during ALL traversal, including inside `$`-call bodies that become segments. OXC needs to either:

**Option A (Recommended): Filter entry-module imports post-hoc**
After traversal, determine which framework imports are actually used in entry module code (not in segments). This avoids changing the traversal architecture.

**Option B: Scope-track during traversal**
Track a `in_dollar_body` flag and only set `import_tracker` flags when NOT inside a dollar body. Complex because the tracker is used in many places.

**Option C: DCE-based approach (matches SWC)**
After all transforms, run a DCE/unused-import-removal pass on the entry module to strip imports that are no longer referenced. This is what SWC does via `simplify::simplifier`.

Recommendation: **Option C** is most robust and matches SWC's approach. However, it requires implementing or leveraging OXC's minifier. **Option A** is simpler and may be sufficient: after `exit_program`, scan the non-import statements to see which identifiers are actually referenced, and only emit imports for those.

#### B. Segment Module Import Ordering

The current string-based approach (`body_code.contains(...)`) in `code_move.rs` doesn't produce correct ordering. Need to match SWC's sorted `local_idents` approach.

Key insight: SWC's segment import order is determined by `IdentCollector::get_words()` which returns identifiers sorted by `(Atom, SyntaxContext)`. In OXC, since we build segment code as strings, we need to:
1. Collect all identifiers used in the segment body
2. Sort them alphabetically
3. Look up each identifier in the collected imports to determine its source
4. Emit imports in that sorted order

#### C. Specifier Merging

SWC preserves original multi-specifier imports by passing them through the fold unchanged. For non-Qwik imports, OXC currently preserves them in the `old_body` pass-through. The issue is with Qwik core non-dollar specifiers that OXC re-emits individually.

For entry module Qwik core imports, group specifiers from the same source into a single import statement. This is straightforward post-processing.

### Pattern: Entry Module Body Order

SWC's entry module body ordering:
```
[synthetic imports - framework Qrl-suffixed imports]
[extra_top_items - lazy imports, hoisted fns, hoisted strings]
[original module body - with Qwik imports stripped, transforms applied]
[extra_bottom_items - synthetic exports]
```

OXC's current entry module body ordering:
```
[ALL framework imports - including segment-only ones]
[non-dollar Qwik core specifier imports]
[lazy imports interspersed with original body]
[original non-Qwik imports in original position]
```

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Unused import detection | Full scope analysis from scratch | Scan import specifier names against identifier occurrences in non-import statements | Edge cases in scope analysis are numerous |
| Import sorting | Custom sort algorithm | Simple alphabetical sort of specifier names within each source group | SWC uses `local_idents.sort()` which is alphabetical |
| AST-level import merging | Complex AST manipulation to merge imports | String-level merging during code_move.rs string construction | Segment modules are already string-built |

## Common Pitfalls

### Pitfall 1: Conflating Entry Module and Segment Imports
**What goes wrong:** Setting global flags for imports needed by segment bodies causes those imports to leak into the entry module.
**Why it happens:** OXC's `import_tracker` is a single global struct with boolean flags, with no scoping for "where is this needed."
**How to avoid:** Either scope the flags (complex) or post-process to remove unused imports from the entry module (simpler).
**Warning signs:** Extra `import { _jsxSorted }` or `import { useStore }` lines in entry module output.

### Pitfall 2: Import Ordering Assumptions
**What goes wrong:** Assuming SWC uses insertion order or source-based grouping for imports.
**Why it happens:** SWC actually uses `local_idents.sort()` (alphabetical by Atom) for segment modules and insertion order for synthetic entry-module imports.
**How to avoid:** Match SWC's exact ordering: synthetic first (insertion order), extra_top_items (BTreeMap/sorted), then original body.
**Warning signs:** Import lines in correct set but wrong position.

### Pitfall 3: Non-Qwik Import Stripping
**What goes wrong:** Non-Qwik imports (mongodb, threejs, etc.) appear in entry module when they should only be in segments.
**Why it happens:** OXC preserves ALL original non-Qwik imports in the entry module body. SWC runs DCE to strip unused ones.
**How to avoid:** Implement unused import removal for the entry module. Check which identifiers from non-Qwik imports are actually referenced in the entry module's non-import code.
**Warning signs:** Third-party imports appearing in entry module when only used inside `component$` bodies.

### Pitfall 4: Specifier Merging vs Splitting
**What goes wrong:** SWC has `import { useStore, mutable } from "@qwik.dev/core"` but OXC has two separate imports.
**Why it happens:** SWC preserves original import declarations and re-emits synthetic ones. OXC always creates one import per specifier.
**How to avoid:** For non-dollar Qwik core specifiers that remain in the entry module, group them by source and emit merged imports.
**Warning signs:** Multiple `import { X } from "@qwik.dev/core"` lines where SWC has one merged line.

### Pitfall 5: QRL Hoisting Inside Functions (Deferred from Phase 4)
**What goes wrong:** QRL calls inside loops are inlined instead of hoisted to variable declarations before the loop.
**Why it happens:** OXC's `hoisted_function_stmts` only injects at module top level, not inside function bodies.
**How to avoid:** Implement function-body-level hoisting scope for QRL declarations.
**Warning signs:** `const X = qrl(...)` before loop in SWC, inline `qrl(...)` inside loop in OXC.

## Code Examples

### SWC Entry Module Assembly (fold_module, lines 2560-2614)
```rust
// SWC's fold_module: Assembly order
fn fold_module(&mut self, node: ast::Module) -> ast::Module {
    // 1. Process module body (fold transforms each item)
    let mut module_body = node.body.into_iter()
        .flat_map(|i| { i.fold_with(self) })
        .collect();

    // 2. Synthetic imports first (insertion order)
    body.extend(
        self.options.global_collect.synthetic.iter()
            .map(|(new_local, import)| create_synthetic_named_import(new_local, &import.source))
    );

    // 3. Extra top items (BTreeMap sorted by Id)
    body.extend(self.extra_top_items.values().cloned());

    // 4. Original module body (transforms applied, unused stripped by DCE)
    body.append(&mut module_body);

    // 5. Extra bottom items
    body.extend(self.extra_bottom_items.values().cloned());

    ast::Module { body, ..node }
}
```

### SWC Segment Import Ordering (code_move.rs::new_module)
```rust
// SWC segment module: Import ordering from sorted local_idents
pub fn new_module(ctx: NewModuleCtx) -> Result<(ast::Module, ...), Error> {
    // 1. _captures import (if needed)
    if has_scoped_idents {
        module.body.push(create_synthetic_named_import(&_captures, ctx.core_module));
    }

    // 2. Imports from SORTED local_idents
    for id in ctx.local_idents {  // local_idents already sorted alphabetically
        if let Some(import) = ctx.global.imports.get(id) {
            // Emit import for this identifier
            module.body.push(create_import(id, import));
        } else if let Some(export) = ctx.global.exports.get(id) {
            // Emit re-export import from entry file
            module.body.push(create_reexport_import(id, export));
        }
    }

    // 3. Extra top items (hoisted fns)
    module.body.extend(ctx.extra_top_items.values().cloned());

    // 4. Export declaration
    module.body.push(create_named_export(expr, ctx.name));
}
```

### SWC IdentCollector Sort (collector.rs, lines 320-324)
```rust
// SWC: local_idents are HashSet then sorted
pub fn get_words(self) -> Vec<Id> {
    let mut local_idents: Vec<Id> = self.local_idents.into_iter().collect();
    local_idents.sort();  // Alphabetical by (Atom, SyntaxContext)
    local_idents
}
```

## Remaining Diff Categories

Analysis of all 156 remaining snapshot diffs:

### Category 1: Import-Only Diffs (9 snapshots)
Files with ONLY import changes (no code diffs):
- `destructure_args_colon_props.snap`
- `destructure_args_colon_props2.snap`
- `example_build_server.snap`
- `example_import_assertion.snap`
- `example_jsx_keyed.snap`
- `example_strip_exports_unused.snap`
- `example_ts_enums_no_transpile.snap`
- `issue_964.snap`
- `should_split_spread_props.snap`

### Category 2: Import + Flag Mismatches (~30 snapshots)
Import ordering/extra imports PLUS flag value differences (0/1/2/3 in `_jsxSorted` calls). The 21 remaining flag mismatches from Phase 5 affect ~30 snapshots.

### Category 3: Import + QRL Hoisting (~15 snapshots)
Import issues PLUS missing QRL hoisting (deferred from Phase 4). Affects:
- `example_component_with_event_listeners_inside_loop.snap` (207 non-import diff lines!)
- `should_transform_nested_loops.snap`
- `should_transform_multiple_event_handlers.snap`/`_case2.snap`
- Other loop-related tests

### Category 4: Import + Non-Component Destructuring (~15 snapshots)
Import issues PLUS `_rawProps` handling for non-component$ exports (deferred from Phase 4). Includes:
- `destructure_args_inline_cmp_*.snap` (3 files)
- `example_props_optimization.snap`
- Various props-related tests

### Category 5: Import + Various Code Diffs (~90 snapshots)
Import issues combined with other code-level diffs:
- Missing `_fnSignal` in non-component exports
- Entry strategy inline mode issues
- Capture differences
- Formatting differences (destructuring, function call formatting)
- `component$()` with no args not converted to `componentQrl()`
- Default export import handling (`default as X` vs `X`)
- Re-export `_auto_` prefix differences
- Lazy import ordering within module body

### Category 6: Major Structural Diffs (~5 snapshots)
Significant structural differences:
- `relative_paths.snap` (159 non-import diff lines) - multi-file test, inline strategy differences
- `example_qwik_react.snap` (189 lines) - React integration differences
- `example_qwik_router_inline.snap` (250 lines) - Complex inline transforms
- `example_mutable_children.snap` (267 lines) - Large mutable children test

## Specific Issue Analysis

### IMP-01: Import Statement Ordering

**Entry module ordering issues:**
1. Lazy imports (`const i_XXX = ...`) interleaved with framework imports instead of being grouped
2. Non-dollar Qwik core specifiers emitted after lazy imports instead of before
3. Original non-Qwik imports preserved in original position instead of being grouped

**Segment module ordering issues:**
1. Framework imports (from `@qwik.dev/core`) emitted in hardcoded order from `code_move.rs`
2. SWC sorts ALL segment imports alphabetically by identifier name
3. `_Fragment` from `@qwik.dev/core/jsx-runtime` should sort before `_jsxSorted` from `@qwik.dev/core` (alphabetically `_F` < `_j`)

### IMP-02: Missing or Extra Imports

**Extra imports in entry module (primary issue):**
- ALL `import_tracker` flags emit into entry module, even segment-only ones
- Non-dollar Qwik core specifiers (`useStore`, `mutable`) always re-emitted even when only used in segments
- Non-Qwik imports (`mongodb`, `threejs`, `leaflet`) preserved when only used in segments

**Missing imports in segments:**
- Some framework imports may be missing when string-based detection (`body_code.contains()`) fails to detect them

### IMP-03: Import Specifier Merging

SWC preserves original multi-specifier imports. Cases found in golden output:
- `import { useStore, mutable } from "@qwik.dev/core"` (4 snapshots)
- `import { wrap, useEffect } from "@qwik.dev/core"` (1 snapshot)
- `import { Slot, Fragment } from "@qwik.dev/core"` (1 snapshot)
- `import { componentQrl, inlinedQrl, useStore, useLexicalScope } from "@qwik.dev/core"` (1 snapshot)

These come from original import declarations that SWC passes through unchanged (non-dollar specifiers that are used in entry module code).

### IMP-04: Relative Import Paths

The `relative_paths.snap` test has 159 non-import diff lines, suggesting deeper structural issues beyond just path formatting. Need to investigate whether this is a path resolution bug or an entry strategy difference.

## Deferred Items (Now In Scope)

### QRL Hoisting (from Phase 4, decision 04-02)
**What:** SWC hoists QRL calls to `const` declarations before loops. OXC inlines them.
**Why deferred:** OXC's `hoisted_function_stmts` only injects at module top level.
**Impact:** ~22 diff lines across ~6 snapshots.
**Fix approach:** Implement function-body-level hoisting. Track hoisting scopes per function/arrow, inject QRL declarations before the first loop that references them.

### use*() Return Value Destructuring (from Phase 4, decision 04-03)
**What:** Non-component exports with destructured params (`({ data })`) should be rewritten to `(_rawProps)` with signal wrapping.
**Why deferred:** Only 2 test fixtures affected.
**Impact:** ~59 `_rawProps` diff lines across ~15 snapshots.
**Fix approach:** Apply the same props destructuring transform used for `component$` to non-component default exports that have destructured parameters.

### should_extract_single_qrl_2 Naming (from Phase 3, decision 03-03)
**What:** Dedup suffix `_1` applied to wrong segment due to bottom-up traverse order.
**Impact:** 1 snapshot.
**Fix approach:** May need special-case handling for traverse order in segment naming.

### Remaining Flag Mismatches (from Phase 5)
**What:** 21 flag mismatches: 10 over-aggressive mutability, 8 scope analysis, 3 edge cases.
**Impact:** ~30 snapshots.
**Fix approach:** Would require scope analysis improvements. May need to accept these as known differences or implement targeted fixes.

## Open Questions

1. **DCE approach**: Should OXC implement a general-purpose DCE/unused-import-removal pass (like SWC's simplifier), or should it use a targeted approach that just checks whether import specifiers are referenced in non-import code?
   - What we know: SWC uses `simplify::simplifier` with DCE. OXC has no equivalent.
   - What's unclear: Whether OXC's `oxc_minifier` or similar tools could be leveraged.
   - Recommendation: Start with a targeted unused-import scanner. Full DCE is out of scope.

2. **Flag mismatches**: Should the 21 remaining flag mismatches be fixed or accepted?
   - What we know: They require scope analysis improvements to distinguish globals from locals.
   - What's unclear: How much effort is required vs how many tests they block from passing.
   - Recommendation: Attempt targeted fixes for the most common patterns; accept remaining as known diffs if the effort is disproportionate.

3. **Inline entry strategy**: Several tests use inline mode with different import patterns. Are these import issues or deeper inline-strategy bugs?
   - What we know: `example_inlined_entry_strategy.snap` has 3 non-import diff lines, suggesting minor issues.
   - Recommendation: Address after fixing segment strategy issues; may resolve naturally.

## Sources

### Primary (HIGH confidence)
- SWC optimizer source: `crates/swc-optimizer/core/src/transform.rs` (fold_module, ensure_core_import, extra_top_items)
- SWC optimizer source: `crates/swc-optimizer/core/src/code_move.rs` (new_module, local_idents ordering)
- SWC optimizer source: `crates/swc-optimizer/core/src/collector.rs` (IdentCollector, get_words sort)
- SWC optimizer source: `crates/swc-optimizer/core/src/parse.rs` (simplifier/DCE invocation)
- OXC optimizer source: `crates/qwik-optimizer-oxc/src/transform.rs` (exit_program, import_tracker)
- OXC optimizer source: `crates/qwik-optimizer-oxc/src/code_move.rs` (string-based segment emission)
- All 162 snapshot diffs: `git diff -- crates/qwik-optimizer-oxc/tests/snapshots/`

### Secondary (MEDIUM confidence)
- STATE.md decisions: QRL hoisting deferral, props destructuring deferral, flag mismatch counts

## Metadata

**Confidence breakdown:**
- Entry module import scoping: HIGH - clearly understood from SWC source comparison
- Segment module import ordering: HIGH - SWC's sorted local_idents approach is explicit in code
- Specifier merging: HIGH - SWC's pass-through of original imports is clear
- QRL hoisting fix approach: MEDIUM - approach understood but implementation complexity uncertain
- Remaining flag fixes: LOW - scope analysis improvements are complex and uncertain
- Complete 0-diff target: LOW - large number of diverse code-level diffs may require more phases

**Research date:** 2026-02-20
**Valid until:** Stable (codebase is under active development but patterns are established)
