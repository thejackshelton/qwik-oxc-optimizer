# Phase 12: Signal Wrapping Gaps - Research

**Researched:** 2026-02-23
**Domain:** JSX signal reactivity transforms (_fnSignal, _wrapProp) in OXC Qwik optimizer
**Confidence:** HIGH (based on direct SWC source code comparison and snapshot diff analysis)

## Summary

Phase 12 addresses the single largest category of remaining semantic diffs: missing, incorrect, or misplaced `_fnSignal` and `_wrapProp` signal wrapping calls. Through systematic analysis of all 99 snapshot diffs, I identified **32 files** with signal-wrapping-related differences (23 with `_fnSignal` diffs, 14 with `_wrapProp` diffs, 5 overlapping).

The root causes cluster into **7 distinct bug categories**, not 32 independent issues. Most failures trace back to a few systematic gaps in the OXC port's signal detection and wrapping logic compared to SWC's `create_synthetic_qqsegment` / `convert_to_getter` / `convert_to_signal_item` pipeline. The categories are well-defined and can be addressed systematically.

**Primary recommendation:** Fix root causes in priority order -- each root cause fix resolves multiple test failures simultaneously. Start with the `is_used_as_object` check (missing in OXC), which is the single biggest gap affecting ~10+ tests.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- Fix ALL semantic diffs -- only OXC codegen aesthetic differences (shorthand, whitespace) are accepted
- No "won't fix" categories -- every semantic diff must be addressed across the 3 phases
- Tests with multiple overlapping categories: fixing wrapping in phase 12 may cascade-resolve some captures/DCE diffs too
- _fnSignal: Missing or incorrect _fnSignal() wrapping in JSX expressions (~23 tests)
- _wrapProp: Missing or incorrect _wrapProp() wrapping in component props (~14 tests)
- Many tests have both -- these are interrelated transforms
- Fixing wrapping also changes capture content (side effect -- benefits phase 13)

### Claude's Discretion
- Technical approach to fixing wrapping gaps (per-test vs systematic analysis)
- Research methodology (SWC reference comparison depth)
- Plan structure and task breakdown

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope. Captures/DCE and small categories are already scoped as phases 13-14.
</user_constraints>

## Architecture Patterns

### SWC Signal Wrapping Pipeline

SWC's signal wrapping follows a 3-stage pipeline in `transform.rs`:

```
1. create_synthetic_qqsegment(expr, accept_call_expr)
   |-> Collects ALL idents via IdentCollector
   |-> Partitions decl_stack into (Var scoped, invalid_decl)
   |-> Checks: global? -> contains_side_effect. invalid_decl? -> (None, false). Not in decl? -> (None, false)
   |-> compute_scoped_idents -> (sorted deps, is_const)
   |-> Side effect? -> (None, scoped_idents.is_empty())
   |-> Simple ident? -> (None, is_const)
   |-> Non-const call/template? -> (None, false)
   |-> obj.prop with ident obj? -> make_wrap(_wrapProp, obj, prop)  [KEY PATTERN]
   |-> Otherwise -> convert_inlined_fn (produces _fnSignal)

2. convert_to_getter(expr) -- for JSX props (accept_call_expr=true)
   |-> Calls create_synthetic_qqsegment
   |-> If _fnSignal result: hoist arrow fn -> _hfN const
   |-> Returns (expr, is_const) for const/var classification

3. convert_to_signal_item(expr, const_idents) -- for JSX children (accept_call_expr=false)
   |-> If call expr -> set jsx_mutable, return None (skip wrapping)
   |-> If const expr -> return None
   |-> Calls create_synthetic_qqsegment
   |-> If !is_const -> set jsx_mutable
```

### OXC Signal Wrapping Pipeline (current)

OXC's equivalent pipeline in `jsx_transform.rs`:

```
1. detect_signal_wrap(expr) -> SignalWrapResult
   |-> X.value -> WrapPropSignal
   |-> _rawProps.prop / props.prop -> WrapPropNamed
   |-> Destructured prop ident -> WrapPropNamed
   |-> localVar.prop (in const_bindings, not import) -> WrapPropNamed
   |-> Otherwise -> None

2. For props: if None from above, check contains_function_call
   |-> collect_reactive_deps -> (deps, has_non_reactive)
   |-> If deps && !has_non_reactive -> build_fn_signal_wrapping (_fnSignal)
   |-> Otherwise -> var_props (unwrapped)

3. For children: same pattern as props but different const/var classification
```

### Key Differences Between SWC and OXC

The fundamental architectural difference: **SWC uses `decl_stack` (full scope tracking via Fold trait)** while **OXC uses `const_bindings` (a HashSet of known const declarations)**. SWC knows the exact scope and constness of every identifier; OXC approximates this with a simpler mechanism. This creates several gap categories.

## Root Cause Analysis: 7 Bug Categories

### Bug 1: Missing `is_used_as_object` Check (~10 tests)
**Confidence:** HIGH (verified in SWC inlined_fn.rs)

**SWC behavior:** `convert_inlined_fn()` (inlined_fn.rs:50-56) checks `is_used_as_object_or_call()` before wrapping. An expression is only wrapped with `_fnSignal` if at least one scoped identifier is used as an object of a member expression (directly or inside `||` / parenthesized expressions). If the identifier is used only as a standalone reference (e.g., `results[i]` where `i` is an index, not an object), SWC returns `(None, is_const)` -- no wrapping.

**OXC behavior:** OXC's `collect_reactive_deps` collects ALL identifiers that are not imports/globals as potential reactive deps. It does NOT check whether the dep is actually used as an "object" (i.e., has `.property` access). This means expressions like `results[i]` where `results` is a store (reactive) and `i` is a loop variable get wrapped with `_fnSignal` incorrectly (e.g., `_fnSignal(_hf0, [i, results], "p1[p0]")` when SWC just leaves `results[i]` bare).

**Affected tests:**
- `example_component_with_event_listeners_inside_loop` -- `results[i]`, `results[key]` should NOT be _fnSignal-wrapped
- `should_wrap_logical_expression_in_template` -- `(count || count2).value` -- SWC wraps because both are used as objects (via `.value`), but OXC fails to wrap because... actually this IS a wrapping case. Let me re-examine.

Wait -- the `should_wrap_logical_expression_in_template` diff shows SWC wraps `(count || count2).value` with `_fnSignal(_hf0, [count, count2], "(p0||p1).value")` but OXC does NOT wrap it. This is actually the opposite: OXC is MISSING wrapping where SWC does it. The issue is that `(count || count2).value` has its `.value` access on the result of a logical OR, not directly on an identifier. OXC's `detect_signal_wrap` only matches `Identifier.value` patterns, not `(expr || expr).value`.

**Revised analysis for Bug 1:** The `is_used_as_object` check is about preventing wrapping when deps are NOT used as member expression objects. The `ObjectUsageChecker` in SWC specifically looks through `||` (LogicalOr) and parenthesized expressions. OXC has no equivalent check.

**Tests where missing `is_used_as_object` causes EXTRA wrapping (OXC wraps, SWC doesn't):**
- `example_component_with_event_listeners_inside_loop` -- computed member `results[i]` should not wrap

### Bug 2: Computed Member Expressions Not Triggering _fnSignal (~5 tests)
**Confidence:** HIGH

**SWC behavior:** In `create_synthetic_qqsegment`, when `obj.prop` is detected AND `obj` is a simple ident, SWC generates `_wrapProp(obj, "prop")`. But for computed access like `results[i]`, `(count || count2).value`, or multi-level chains with computed parts, SWC falls through to `convert_inlined_fn` which generates `_fnSignal`.

**OXC behavior:** `detect_signal_wrap` only handles `StaticMemberExpression` and `ComputedMemberExpression` with simple identifier objects. It does NOT handle:
1. Parenthesized expressions as member objects: `(count || count2).value`
2. The case where SWC's `is_used_as_object` recursion through logical OR discovers the ident

**Affected tests:**
- `should_wrap_logical_expression_in_template` -- `(count || count2).value` not detected as reactive

### Bug 3: `_fnSignal` Dep Ordering Mismatch (~8 tests)
**Confidence:** HIGH (direct diff comparison)

**SWC behavior:** `compute_scoped_idents` collects deps from `decl_stack` using a `HashSet` then sorts alphabetically. The `scoped_idents` output is always alphabetically sorted by identifier name.

**OXC behavior:** `collect_reactive_deps` collects deps in encounter order (left-to-right through the expression tree). For `_rawProps` (or props param), it appears as a primary dep; for local variables, they appear as co-reactive deps appended after primary deps. The final order is: primary deps (encounter order) + local deps (encounter order).

**Result:** When SWC produces `_fnSignal(_hf0, [fromLocal, props], ...)` (alphabetical), OXC produces `_fnSignal(_hf0, [props, fromLocal], ...)` (encounter order with props first as "primary"). This also cascades into different `pN` parameter assignments in the hoisted function, changing the function body.

**Affected tests (dep ordering):**
- `example_props_wrapping2` -- `[fromLocal, props]` vs `[props, fromLocal]`
- `example_props_wrapping_children2` -- same pattern
- All tests with mixed props + local deps in _fnSignal

### Bug 4: Missing _fnSignal for Non-`.value` Expressions on Props (~6 tests)
**Confidence:** HIGH

**SWC behavior:** `create_synthetic_qqsegment` wraps ANY expression where scoped idents are used as member expression objects. For example, `_rawProps.description ?? ""` where `_rawProps` is a scoped variable gets wrapped as `_fnSignal`.

**OXC behavior:** For expressions like `(_rawProps.description ?? "") && "description" in _rawProps.other ? \`Hello ${counter.value}\` : \`Bye ${counter.value}\``, OXC's `collect_reactive_deps` correctly identifies `_rawProps` and `counter` as deps, but `contains_function_call` returns false... Let me check: the actual issue in `example_issue_33443` is that OXC does NOT wrap the expression. Looking at the diff: SWC wraps `(p0.description??"")&&"description"in p0.other?\`Hello ${p1.value}\`:\`Bye ${p1.value}\`` with `_fnSignal`. OXC outputs the expression bare.

The issue: OXC's `collect_reactive_deps_inner` for template literals -- it does NOT recurse into `TemplateLiteral` expressions. Template expressions like `` `Hello ${counter.value}` `` contain reactive deps inside their expressions array, but `collect_reactive_deps_inner` has no `Expression::TemplateLiteral` match arm -- it falls through to the empty `_ => {}` case.

**Tests affected:**
- `example_issue_33443` -- complex props expression with template literals
- `should_wrap_store_expression` -- ternary with store access (missing _fnSignal)
- `ternary_prop` -- ternary with signal.value in prop position

### Bug 5: _hf Counter Deduplication vs Per-Use Generation (~5 tests)
**Confidence:** HIGH

**SWC behavior:** `hoist_fn_signal_call` in SWC uses `hoisted_fn_signals: HashMap<String, Id>` to deduplicate hoisted functions by their rendered body string. If two different expressions produce the same function body (e.g., two `store.errors.test` accesses on different stores), SWC reuses the same `_hf0` for both.

**OXC behavior:** `build_fn_signal_wrapping` always increments `hoisted_fn_counter`, creating a new `_hfN` for every wrapping call regardless of body equality. This produces `_hf0`, `_hf1`, `_hf2`, `_hf3`, `_hf4` where SWC would produce `_hf0` (reused 5 times).

**Affected tests:**
- `should_wrap_prop_from_destructured_array` -- 5 `store.errors.test` accesses should share `_hf0`
- `example_getter_generation` -- same function body on different deps should share counter
- All tests with repeated identical hoisted function bodies

### Bug 6: Missing _fnSignal Wrapping for Inline Component Props with Destructured Args (~3 tests)
**Confidence:** HIGH

**SWC behavior:** When a component has destructured parameters like `({ data }: { data: any })`, SWC replaces the destructuring with `_rawProps` and wraps prop access like `_rawProps.data.selectedOutputDetail === "options"` with `_fnSignal`. The destructured variable `data` becomes `_rawProps.data` in the transformed output.

**OXC behavior:** OXC does NOT perform this `_rawProps` replacement for certain inline component patterns (non-`component$` arrow functions used as default exports). The destructured `{ data }` pattern is preserved, and `data.selectedOutputDetail` is used directly without signal wrapping.

**Root cause:** The props destructuring -> `_rawProps` replacement is only applied in certain contexts. For `export default ({ data }) => {...}` patterns (inline components without `component$`), the destructuring-to-_rawProps rewrite is not triggered, so signal wrapping never fires.

**Affected tests:**
- `destructure_args_inline_cmp_block_stmt` -- `({ data })` not rewritten to `_rawProps`
- `destructure_args_inline_cmp_block_stmt2` -- `(props: { data: any })` not rewritten
- `destructure_args_inline_cmp_expr_stmt` -- same as block_stmt

### Bug 7: Prop Ordering in JSX (const/var classification) (~8 tests)
**Confidence:** HIGH

**SWC behavior:** Props are classified in source order but then placed into `const_props` or `var_props` buckets. Within each bucket, order is preserved from source. The final output interleaves these buckets in a specific way.

**OXC behavior:** Props are also classified into const/var buckets, but the ordering within buckets sometimes differs from SWC because:
1. Signal-wrapped props (via `detect_signal_wrap` or `build_fn_signal_wrapping`) get `continue;` early, which means they bypass the normal ordering logic
2. The classification decisions sometimes differ (const vs var), causing props to appear in different buckets

**Affected tests:** Many tests show reordered props (e.g., `"props-only"` before `"props"` or after). This is a secondary effect of other wrapping bugs -- when the wrapping is correct, the const/var classification also becomes correct, which fixes the ordering.

## Non-Signal-Wrapping Diffs in the 32 Files

Several files in the 32-file set have diffs that are NOT signal-wrapping bugs:

1. **Import ordering** (many tests): `_fnSignal` before `_wrapProp` or vice versa. This is import sort order, not wrapping logic. Should resolve when import set is correct.

2. **_rawProps diffs** in 10 files: Some are wrapping-related (Bug 6), others are props destructuring issues (phase 13 scope).

3. **Aesthetic diffs**: OXC shorthand `{ signal }` vs SWC `{ signal: signal }`, line wrapping differences. These are accepted.

4. **Other category diffs** mixed into wrapping test files: ctxKind, entry field, paramNames/captureNames ordering -- these are phase 14 scope.

## Affected Test Classification

### Pure Signal Wrapping Bugs (Phase 12 scope -- 25 tests)

| Test | Bug(s) | Wrapping Issue |
|------|--------|----------------|
| destructure_args_inline_cmp_block_stmt | 6 | Missing _rawProps rewrite for inline cmp |
| destructure_args_inline_cmp_block_stmt2 | 6 | Same pattern, named props param |
| destructure_args_inline_cmp_expr_stmt | 6 | Same pattern, expression body |
| example_component_with_event_listeners_inside_loop | 1 | `results[i]` incorrectly wrapped |
| example_derived_signals_children | 7 | Import ordering only (signal wrapping correct) |
| example_derived_signals_cmp | 7 | Import ordering + prop ordering |
| example_derived_signals_div | 7 | Import ordering + prop ordering |
| example_derived_signals_multiple_children | 7 | Import ordering only |
| example_functional_component_2 | 1 | `btn.name` in loop child should be `_wrapProp(btn, "name")` |
| example_getter_generation | 4,5 | Missing wrapping for `store.stuff + 12`, `signal.formData?.get("username")`; wrong _hf counter |
| example_immutable_analysis | 4 | Missing _fnSignal for `{foo: "bar", baz: p0.count ? true : false}` |
| example_issue_33443 | 4 | Missing _fnSignal for template literal expression |
| example_issue_4438 | 2 | Extra _fnSignal (tagged template $localize wrapping) |
| example_props_optimization | 3,4,5 | Multiple: dep ordering, missing wrapping, hf reuse |
| example_props_wrapping | 3,7 | Dep ordering, prop ordering |
| example_props_wrapping2 | 3,7 | Dep ordering, prop ordering, pN naming |
| example_props_wrapping_children | 7 | Import/prop ordering |
| example_props_wrapping_children2 | 3,7 | Dep ordering, prop ordering |
| should_merge_attributes_with_spread_props | 4 | Missing _fnSignal for `[props.class, "component"]` |
| should_merge_attributes_with_spread_props_before_and_after | 4 | Same pattern |
| should_transform_multiple_event_handlers | 2 | Missing _fnSignal for `item.value.id` |
| should_transform_multiple_event_handlers_case2 | 2 | Same pattern |
| should_transform_nested_loops | 5 | _hf counter differences |
| should_wrap_logical_expression_in_template | 2 | `(count || count2).value` not wrapped |
| should_wrap_prop_from_destructured_array | 5 | _hf dedup (5 identical bodies, 5 counters) |
| should_wrap_store_expression | 4 | Missing _fnSignal for `panelStore.active ? "yes" : "no"` |
| ternary_prop | 4 | Missing _fnSignal for `toggleSig.value ? true : undefined` |

### Tests with Mixed Wrapping + Non-Wrapping Diffs (will partially improve)

| Test | Wrapping Bug | Other Diffs |
|------|-------------|-------------|
| example_parsed_inlined_qrls | _wrapProp missing | Pre-transformed QRL test (known limitation) |
| relative_paths | _wrapProp missing | Multi-input test entry ordering |
| example_strip_client_code | _wrapProp placement | Code stripping differences |
| example_missing_custom_inlined_functions | Import ordering only | Diagnostic emission |
| example_mutable_children | Import ordering only | Brace insertion (aesthetic) |

## Common Pitfalls

### Pitfall 1: Dep Collection Order vs Alphabetical Sort
**What goes wrong:** OXC collects deps in expression-tree traversal order; SWC sorts them alphabetically. This causes different `pN` assignments, different function bodies, different `_hf_str` values.
**Why it happens:** SWC uses `HashSet -> Vec -> sort()` in `compute_scoped_idents`; OXC uses `Vec` with encounter order in `collect_reactive_deps`.
**How to avoid:** Sort deps alphabetically by `root_name` after collection, before building the `_fnSignal` call.
**Warning signs:** `_hf0 = (p0, p1) => p0 + p1.fromProps` vs `_hf0 = (p0, p1) => p1 + p0.fromProps` -- same expression, different param assignments.

### Pitfall 2: Object Usage Check Prevents False Wrapping
**What goes wrong:** OXC wraps expressions where SWC returns `(None, is_const)` because the scoped ident is not used as a member expression object.
**Why it happens:** SWC's `is_used_as_object_or_call` check (inlined_fn.rs) prevents wrapping when no scoped ident has `.property` access. OXC has no equivalent.
**How to avoid:** Implement the `is_used_as_object` check. An ident is "used as object" if it appears as `obj` in a `MemberExpression`, including through `||` and parenthesized wrappers.
**Warning signs:** `_fnSignal(_hf0, [i, results], "p1[p0]")` where the correct output is bare `results[i]`.

### Pitfall 3: Expression Types Not Recursed In collect_reactive_deps
**What goes wrong:** Template literals, tagged templates, array expressions, and `in` operator expressions are not traversed by `collect_reactive_deps_inner`, causing deps inside them to be missed.
**Why it happens:** The `match expr` in `collect_reactive_deps_inner` only handles: StaticMemberExpression, Identifier, BinaryExpression, ConditionalExpression, UnaryExpression, ObjectExpression, ParenthesizedExpression. Missing: TemplateLiteral, ArrayExpression, ComputedMemberExpression (partial), TaggedTemplateExpression.
**How to avoid:** Add match arms for all expression types that can contain reactive identifiers.
**Warning signs:** Expressions with template literals or array literals that contain reactive deps are not wrapped.

### Pitfall 4: _hf Counter Not Deduplicated
**What goes wrong:** Each `build_fn_signal_wrapping` call gets a unique `_hfN` even when the function body is identical to a previous one.
**Why it happens:** SWC's `hoisted_fn_signals: HashMap<String, Id>` deduplicates by rendered body string; OXC always increments `hoisted_fn_counter`.
**How to avoid:** Before incrementing the counter, check if the rendered body already exists in a dedup map. If so, reuse the existing `_hfN`.
**Warning signs:** Multiple `const _hfN` declarations with identical function bodies.

### Pitfall 5: `contains_function_call` Prevents Valid Wrapping
**What goes wrong:** `contains_function_call` returns true for optional chaining like `signal.formData?.get("username")`, preventing _fnSignal wrapping.
**Why it happens:** Optional chaining desugars to include a call expression. SWC's `create_synthetic_qqsegment` has `accept_call_expr=true` for prop context, allowing calls. OXC's prop path uses `!contains_function_call(&value)` as a gate, which blocks valid wrapping.
**How to avoid:** For prop context (not children), pass the equivalent of `accept_call_expr=true` to allow function calls inside wrapped expressions.
**Warning signs:** `signal.formData?.get("username")` left unwrapped when SWC wraps it.

## Code Examples

### Fix 1: Dep Sorting (Bug 3)

In `collect_reactive_deps()`, after merging local deps into primary deps, sort by `root_name`:

```rust
// After merging local_deps into primary_deps:
primary_deps.sort_by(|a, b| a.root_name.cmp(&b.root_name));
// Re-assign param names after sorting
for (i, dep) in primary_deps.iter_mut().enumerate() {
    dep.param_name = format!("p{}", i);
}
```

### Fix 2: _hf Deduplication (Bug 5)

Add a dedup map to `ImportTracker`:

```rust
pub hoisted_fn_dedup: HashMap<String, usize>,  // body_str -> hf_index
```

In `build_fn_signal_wrapping`, before creating new hf:

```rust
// Check for existing identical hoisted function
let body_key = format!("({}) => {}", params_str, body_for_fn);
if let Some(&existing_index) = tracker.hoisted_fn_dedup.get(&body_key) {
    // Reuse existing _hfN
    let hf_name = format!("_hf{}", existing_index);
    // ... build call referencing existing _hfN, skip hoisted_stmts push
} else {
    let hf_index = tracker.hoisted_fn_counter;
    tracker.hoisted_fn_counter += 1;
    tracker.hoisted_fn_dedup.insert(body_key, hf_index);
    // ... existing code
}
```

### Fix 3: is_used_as_object Check (Bug 1)

Add a check before wrapping in both prop and children contexts:

```rust
fn is_any_dep_used_as_object(expr: &Expression<'_>, dep_names: &[&str]) -> bool {
    match expr {
        Expression::StaticMemberExpression(member) => {
            // Check if the object is (directly or through || / parens) one of our deps
            if is_dep_object(&member.object, dep_names) {
                return true;
            }
            is_any_dep_used_as_object(&member.object, dep_names)
        }
        Expression::ComputedMemberExpression(member) => {
            if is_dep_object(&member.object, dep_names) {
                return true;
            }
            is_any_dep_used_as_object(&member.object, dep_names)
                || is_any_dep_used_as_object(&member.expression, dep_names)
        }
        Expression::BinaryExpression(bin) => {
            is_any_dep_used_as_object(&bin.left, dep_names)
                || is_any_dep_used_as_object(&bin.right, dep_names)
        }
        // ... recurse into all expression types
    }
}

fn is_dep_object(expr: &Expression<'_>, dep_names: &[&str]) -> bool {
    match expr {
        Expression::Identifier(ident) => dep_names.contains(&ident.name.as_str()),
        Expression::ParenthesizedExpression(p) => is_dep_object(&p.expression, dep_names),
        Expression::LogicalExpression(l) if l.operator == LogicalOperator::Or => {
            is_dep_object(&l.left, dep_names) || is_dep_object(&l.right, dep_names)
        }
        _ => false,
    }
}
```

### Fix 4: Template Literal Recursion (Bug 4)

Add match arms in `collect_reactive_deps_inner`:

```rust
Expression::TemplateLiteral(tpl) => {
    for expr in &tpl.expressions {
        collect_reactive_deps_inner(
            expr, destructured_props, collected_imports,
            primary_deps, local_deps, seen,
            has_non_reactive_non_const, props_param_name,
        );
    }
}
Expression::ArrayExpression(arr) => {
    for elem in &arr.elements {
        match elem {
            ArrayExpressionElement::SpreadElement(s) => {
                collect_reactive_deps_inner(&s.argument, ...);
            }
            ArrayExpressionElement::Elision(_) => {}
            _ => {
                if let Some(expr) = elem.as_expression() {
                    collect_reactive_deps_inner(expr, ...);
                }
            }
        }
    }
}
Expression::ComputedMemberExpression(member) => {
    // Recurse into both object and computed key
    collect_reactive_deps_inner(&member.object, ...);
    collect_reactive_deps_inner(&member.expression, ...);
}
Expression::TaggedTemplateExpression(tagged) => {
    collect_reactive_deps_inner(&tagged.quasi, ...);
    // Also recurse template expressions
}
```

### Fix 5: accept_call_expr for Props (Bug 5 / Pitfall 5)

In the prop wrapping path, bypass `contains_function_call` check:

```rust
// Current (wrong):
} else if !contains_function_call(&value) {
    let (deps, has_non_reactive) = collect_reactive_deps(...);

// Fixed:
} else {
    // For props, SWC uses accept_call_expr=true, allowing function calls
    // Only skip if it's an arrow function (SWC also bails on arrows)
    let should_check_fn_signal = !matches!(&value, Expression::ArrowFunctionExpression(_));
    if should_check_fn_signal {
        let (deps, has_non_reactive) = collect_reactive_deps(...);
```

Note: For children context, `accept_call_expr=false` is correct -- keep the existing `!contains_function_call` gate for the children code path.

## Implementation Strategy

### Recommended Task Breakdown

**Plan 1: Core wrapping logic fixes (Bugs 1, 2, 3, 4, 5)**
1. Add dep sorting (alphabetical by root_name) in `collect_reactive_deps`
2. Add `is_used_as_object` check before `_fnSignal` wrapping
3. Add missing expression type recursion in `collect_reactive_deps_inner` (TemplateLiteral, ArrayExpression, ComputedMemberExpression, TaggedTemplateExpression)
4. Implement `_hf` deduplication via body-string HashMap
5. Fix `accept_call_expr` equivalent for prop context (allow calls in prop _fnSignal)

**Plan 2: Inline component _rawProps + remaining edge cases (Bug 6 + cleanup)**
1. Extend props destructuring -> _rawProps rewrite to inline component patterns
2. Fix any remaining per-test issues discovered after Plan 1

### Expected Cascade Effects
- Fixing dep sorting (Bug 3) will fix ~8 tests' _fnSignal bodies and dep arrays
- Fixing is_used_as_object (Bug 1) will fix ~3 tests where wrapping is incorrectly applied
- Fixing template literal recursion (Bug 4) will fix ~6 tests where wrapping is missing
- Fixing _hf dedup (Bug 5) will fix ~5 tests with counter numbering issues
- Prop ordering (Bug 7) should largely self-resolve when wrapping is correct

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Dep alphabetical sort | Custom sort logic | `primary_deps.sort_by(key)` then reassign `param_name` | Simple, matches SWC exactly |
| Object usage detection | Full scope analysis | Simple recursive expression walker | SWC's checker is ~50 lines, not complex |
| _hf deduplication | Counter-based naming | HashMap<String, usize> keyed by rendered body | SWC uses exactly this pattern |

## Open Questions

1. **LogicalExpression vs BinaryExpression:** OXC represents `||` as `LogicalExpression` while SWC uses `BinaryExpression` with `BinaryOp::LogicalOr`. Need to verify OXC's AST representation when implementing `is_dep_object` check -- use `LogicalExpression` not `BinaryExpression`.

2. **accept_call_expr edge cases:** SWC allows calls in prop context but not children. The `ReplaceIdentifiers` visitor in SWC aborts on `visit_mut_callee` unless `accept_call_expr=true`. OXC's string-based replacement doesn't have this abort mechanism. Need to verify: does OXC need an equivalent check, or is the string replacement sufficient?

3. **Inline component detection:** For Bug 6, need to verify exactly which patterns trigger `_rawProps` replacement. SWC appears to do this for any function that's used as a JSX component. OXC may need to detect this during the JSX transform when the component tag matches a local function.

## Sources

### Primary (HIGH confidence)
- SWC `transform.rs` lines 567-648: `create_synthetic_qqsegment` implementation
- SWC `inlined_fn.rs` lines 24-295: `convert_inlined_fn`, `is_used_as_object_or_call`, `ObjectUsageChecker`
- SWC `transform.rs` lines 2191-2237: `convert_to_getter`, `convert_to_signal_item`
- SWC `transform.rs` lines 3582-3596: `compute_scoped_idents`
- OXC `jsx_transform.rs` lines 463-1077: `detect_signal_wrap`, `collect_reactive_deps`, `build_fn_signal_wrapping`
- Direct snapshot diff analysis of all 99 differing snapshots

### Secondary (MEDIUM confidence)
- Prior phase decisions documented in STATE.md (decisions [04-01], [05-02], [09-02], [09-05])
- Phase 4 and Phase 9 plan summaries for historical context on wrapping implementation

## Metadata

**Confidence breakdown:**
- Root cause analysis: HIGH - Direct SWC source comparison confirms all 7 bug categories
- Architecture patterns: HIGH - Both codebases are available and thoroughly examined
- Code examples: HIGH - Based on direct SWC reference patterns
- Test classification: HIGH - Every test diff individually examined

**Research date:** 2026-02-23
**Valid until:** Indefinite (both codebases are pinned)
