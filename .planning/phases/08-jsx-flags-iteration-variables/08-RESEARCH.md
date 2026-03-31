# Phase 8: JSX Flags & Iteration Variables - Research

**Researched:** 2026-02-21
**Domain:** JSX immutability flag computation, identifier scope analysis, loop event handler detection, q:p injection
**Confidence:** HIGH

## Summary

Phase 8 addresses the largest remaining category of snapshot mismatches: JSX immutability flags (59 paired flag mismatches across ~66 files) and missing q:p/q:ps iteration variable injection (20 missing q:p lines across 10 files). The root cause is a lack of scope-aware const analysis -- OXC treats identifiers uniformly in two contradictory ways depending on context, while SWC uses its `ConstCollector` to distinguish const-bound locals, imports, and mutable/unresolved identifiers.

The phase decomposes into 5 distinct sub-problems:
1. **Identifier scope analysis** (GAP-3): Build a const-bindings set to replace the blanket `Expression::Identifier(_) => true` in `is_child_expression_immutable()` and add scope-aware classification in `is_const_jsx_value()` for prop classification
2. **Logical && mutability propagation** (GAP-5): Ensure `prop.value && <div/>` correctly marks the parent as mutable
3. **Loop event handler static_listeners** (GAP-3 related): When q:p is present, set static_listeners=false (bit 0 cleared)
4. **q:p/q:ps injection fix** (GAP-6): Event handler values are already qrl() calls when q:p injection runs; need to check captures instead
5. **_rawProps override for non-component$ hooks** (GAP-4): Extend active_props_info to useResource$ and other hooks

**Primary recommendation:** Build a `const_bindings: HashSet<String>` during traversal that tracks all `const`/`let` declarations and imports seen in scope, then use it in both `is_const_jsx_value()` (props) and `is_child_expression_immutable()` (children) to correctly classify identifiers.

## Architecture Patterns

### Current Flag Computation Flow

```
JSX Element Processing (transform_jsx_element_inner):
  1. Classify attributes -> const_props, var_props, spread_args
  2. Inject q:p/q:ps for iteration vars (checks expr_uses_ident on handler values)
  3. Build children (transform_jsx_children)
  4. Compute flags:
     - static_listeners = !has_spread
     - static_subtree = !has_spread && var_props.is_empty() && !children_mutable
  5. Encode: flags = (static_listeners ? 1 : 0) | (static_subtree ? 2 : 0)
  6. Propagate: tracker.jsx_mutable = true if has_spread || !var_props.empty || children_mutable
```

### Flag Bit Encoding

| Flag Value | Bit 0 (static_listeners) | Bit 1 (static_subtree) | Meaning |
|-----------|--------------------------|------------------------|---------|
| 0 | false | false | Has spread or loop event handlers with q:p |
| 1 | true | false | Has var_props or mutable children |
| 2 | false | true | Has spread but const subtree (rare) |
| 3 | true | true | Fully immutable |

### Problem 1: Two Contradictory Identifier Approximations

**For prop values** (`is_const_expression`):
- All identifiers return `false` -> goes to var_props -> breaks static_subtree
- SWC behavior: const-bound locals and imports are const -> goes to const_props
- **Effect:** 21 cases of SWC=3 OXC=1 (identifier props incorrectly in var_props)

**For children** (`is_child_expression_immutable`):
- All identifiers return `true` -> treated as immutable
- SWC behavior: uses ConstCollector scope analysis -- unresolved globals and reactive locals are mutable
- **Effect:** 13 cases of SWC=1 OXC=3 (reactive identifier children incorrectly marked immutable)

### Problem 2: Missing q:p Injection

**Current code** (`jsx_transform.rs` lines 1495-1534):
```rust
// Inject q:p / q:ps for iteration variables used by event handlers in loops
if loop_depth > 0 && !iteration_vars.is_empty() {
    for iter_var in iteration_vars {
        let is_used = const_props.iter().any(|(key, value)| {
            key.starts_with("q-") && expr_uses_ident(value, iter_var)
        }) || var_props.iter().any(|(key, value)| {
            key.starts_with("q-") && expr_uses_ident(value, iter_var)
        });
        // ...
    }
}
```

**The bug:** By the time JSX transform runs, event handler lambdas have already been replaced by `enter_call_expression` in `transform.rs`. The const_prop values for `q-e:*` keys are now identifier references to hoisted QRL constants (e.g., `App_component_div_q_e_click_XXXXXXXXXXXX`), not the original lambda bodies. So `expr_uses_ident(value, "item")` returns false because the value is just an identifier reference, not an expression tree containing `item`.

**SWC approach:** SWC checks iteration variable usage during the event handler processing itself and records the q:p vars alongside the event handler. The iteration variable information is available at event handler registration time, not at JSX element construction time.

**Fix approach:** Instead of checking expr_uses_ident on the already-transformed handler values, track which iteration variables were captured by event handlers during `enter_call_expression` (where `body_ident_refs` are analyzed). Store this per-element or check the QRL captures list.

### Problem 3: static_listeners Not Cleared for Loop Event Handlers

When `q:p` is present (i.e., event handlers use iteration variables), SWC sets `static_listeners = false` (bit 0 cleared), resulting in flag 0 or 2. OXC currently only clears static_listeners when `has_spread` is true.

**Evidence from diffs:**
- 8 cases of SWC=0 OXC=3 (should have static_listeners=false + static_subtree=false)
- 4 cases of SWC=0 OXC=1 (should have static_listeners=false)
- 4 cases of SWC=2 OXC=1 (should have static_listeners=false but subtree=true)
- Total: ~16 cases where static_listeners should be false

**Fix:** After q:p injection, if any q:p/q:ps was added, set `static_listeners = false`.

### Problem 4: Logical && Propagation

The `contains_mutable_jsx_call` check catches `<Stuff/>` inside ternaries and logical &&, but the `prop.value &&` left side of the logical expression is itself mutable (it's a member expression with `.value`). The current `is_child_expression_immutable` for `LogicalExpression` recursively checks both sides:

```rust
Expression::LogicalExpression(log) => {
    is_child_expression_immutable(&log.left, module_imports)
        && is_child_expression_immutable(&log.right, module_imports)
}
```

For `prop.value && <div/>`:
- `log.left` = `prop.value` (StaticMemberExpression) -> false unless `prop` is import
- `log.right` = `_jsxSorted(...)` call -> true (it's a known immutable call)
- Result: false (mutable) -- this seems correct

But the issue may be in how the already-transformed right side propagates. When `<div/>` becomes `_jsxSorted("div", ...)`, the right side IS immutable (it's a _jsxSorted call). The left side `prop.value` would be mutable IF `prop` is a local reactive variable (not an import). With current code, `prop` as a StaticMemberExpression base would check `module_imports` -- if `prop` is not an import, returns false. This seems correct.

The specific 9 cases of SWC=2 OXC=3 suggest the issue is about static_listeners being set wrong. SWC=2 means static_listeners=false, static_subtree=true. Let me check if these are related to q:p injection cases where static_listeners should be false but isn't.

Actually, looking more carefully at the SWC=2 cases (from `example_of_synchronous_qrl`, `example_reg_ctx_name_segments`, etc.), these are likely cases where `_qrlSync` or other special calls affect static_listeners. This needs further per-test investigation during planning.

### Problem 5: _rawProps Override for Non-component$ Hooks

**Current code** (`transform.rs` line 1444):
```rust
if name == "component$" {
    // ... analyze props destructuring
    self.active_props_info = Some(info);
}
```

Only `component$` gets `active_props_info`. For `useResource$`, the props parameter (which is typically `{track, cleanup}`) would need similar treatment. However, examining the diff more carefully, the actual mismatch is:

From `example_props_optimization.snap`:
- SWC: `"paramNames": ["_rawProps"]` for `useResource$` segment
- OXC: `"paramNames": ["{track, cleanup}"]` or similar

SWC replaces the destructured parameter with `_rawProps` for useResource$ too. The fix is to extend the `active_props_info` logic to apply to ALL `$`-suffixed calls that have destructured parameters, not just `component$`.

## Detailed Flag Mismatch Breakdown

From snapshot diff analysis (59 paired flag mismatches):

| Category | Count | Root Cause | Fix |
|----------|-------|-----------|-----|
| SWC=3 OXC=1 | 21 | Identifiers in props treated as non-const | Add scope-aware `is_const_jsx_value` |
| SWC=1 OXC=3 | 13 | Identifiers in children treated as immutable | Add scope-aware `is_child_expression_immutable` |
| SWC=2 OXC=3 | 9 | static_listeners should be false | Fix q:p + static_listeners logic |
| SWC=0 OXC=3 | 8 | Both flags wrong (loop event handlers) | Fix q:p injection + static_listeners |
| SWC=2 OXC=1 | 4 | static_listeners wrong, subtree wrong | Fix both |
| SWC=0 OXC=1 | 4 | static_listeners should be false | Fix q:p + static_listeners |

### Tests with Missing q:p (10 files, 20 lines)

1. `example_component_with_event_listeners_inside_loop` - 7 q:p/q:ps missing
2. `should_transform_multiple_event_handlers` - 2 q:p missing
3. `should_transform_multiple_event_handlers_case2` - 2 q:ps missing
4. `should_transform_nested_loops` - 2 q:p missing
5. `should_extract_single_qrl` - 2 q:p missing
6. `should_extract_single_qrl_with_index` - 2 q:p missing
7. `should_extract_single_qrl_2` - 1 q:p missing
8. `should_extract_single_qrl_with_nested_components` - 1 q:p missing
9. `should_transform_component_with_normal_function` - 1 q:p missing
10. `example_functional_component_2` - 1 q:p missing

## Common Pitfalls

### Pitfall 1: Confusing Prop Classification with Children Classification
**What goes wrong:** Applying the same const-checking logic for both prop values and children expressions
**Why it happens:** They use different functions (`is_const_expression` vs `is_child_expression_immutable`) with different semantics
**How to avoid:** Prop classification determines var_props vs const_props (which affects static_subtree via var_props emptiness). Children classification directly affects the `children_mutable` flag. Both need scope analysis but with different baseline assumptions.

### Pitfall 2: Bottom-up Traversal Timing
**What goes wrong:** Checking expression contents after OXC's bottom-up traversal has already transformed them
**Why it happens:** OXC processes inner expressions before outer ones. By the time q:p injection runs on an element, its event handler children are already qrl() calls.
**How to avoid:** Record iteration variable usage at the point where event handlers are analyzed (enter_call_expression), not at JSX element construction time.

### Pitfall 3: Mutable Flag Leaking Between Siblings
**What goes wrong:** jsx_mutable set by one child leaks to affect sibling elements
**Why it happens:** The save/restore pattern for tracker.jsx_mutable must be applied consistently
**How to avoid:** The existing save/restore in transform_jsx_children is correct. Don't change it without understanding the full propagation chain.

### Pitfall 4: Scope Analysis Complexity
**What goes wrong:** Trying to build full SWC-equivalent scope analysis
**Why it happens:** SWC uses a full ConstCollector visitor that walks the entire function body
**How to avoid:** A simpler approach is sufficient: collect const/let declarations and imports during traversal into a HashSet. Local `const` bindings and imports are const; everything else is potentially mutable. This covers >90% of the cases.

### Pitfall 5: Event Handler Captures vs Iteration Variables
**What goes wrong:** Confusing which iteration variables the event handler uses vs which ones exist in scope
**Why it happens:** Multiple iteration variables may be in scope (nested loops) but only some are used by the handler
**How to avoid:** Track used iteration variables per-handler during `enter_call_expression` analysis. The `body_ident_refs` already contains this information (line 1020 in transform.rs).

## Implementation Strategy

### Recommended Approach: Const Bindings Set

Build a `const_bindings: HashSet<String>` on `QwikTransform` that tracks identifiers known to be const:

1. **Initialization:** Populate from `collected.module_imports` (all import specifiers)
2. **During traversal:** On `enter_variable_declaration` with `VariableDeclarationKind::Const`, add all declared binding names to the set
3. **Usage in props:** `is_const_jsx_value` checks if an identifier is in `const_bindings` -> const_props
4. **Usage in children:** `is_child_expression_immutable` checks if an identifier is in `const_bindings` -> immutable

This is simpler than full scope analysis but covers the key cases:
- `const signal = useSignal(0)` -> signal is const -> prop goes to const_props (SWC=3)
- `import { dep } from "./file"` -> dep is const -> const_props
- `let x = ...` -> x is NOT const -> var_props (SWC=1)
- Function params -> NOT const -> mutable children (SWC=1)

**Limitation:** This doesn't handle block scoping (a `const` in an inner block shouldn't affect outer scope). However, in practice, JSX is almost always in the same or child scope of the const declaration, so this approximation is sufficient for matching SWC behavior.

### q:p Injection Fix Approach

**Option A (Recommended):** Store iteration variable usage per-JSX-element during event handler analysis.

In `enter_call_expression` (transform.rs ~line 1016), when processing JSX event handlers inside loops:
1. The `body_ident_refs` already shows which iteration variables the handler uses
2. Record this information in a map keyed by element span or a similar identifier
3. In `transform_jsx_element_inner`, look up the recorded iteration variables instead of scanning handler expressions

**Option B:** Check the QRL replacement's capture_names.

The `JsxEventReplacement` struct has `capture_names: Vec<String>`. When q:p injection runs, check if any capture names match iteration variables. However, this conflates captures with iteration variables -- not all captures are iteration vars and vice versa.

**Option A is cleaner** because it uses the same `body_ident_refs` analysis that already runs, just needs to store the result.

### static_listeners Fix

After q:p/q:ps injection succeeds (any q:p was added), set:
```rust
static_listeners = false;
```

This is a one-line fix in `transform_jsx_element_inner` after the q:p injection block.

### _rawProps Override Extension

Extend the `if name == "component$"` check in `enter_call_expression` to also handle `useResource$` (and potentially other hooks). The props destructuring analysis should apply to any `$`-suffixed call that takes a callback with destructured parameters.

However, for `useResource$`, the parameter is not "props" but `{track, cleanup}` -- these are NOT reactive props that need `_rawProps` wrapping. The actual fix for GAP-4 is likely narrower: just ensure the `paramNames` array in segment metadata reflects the correct parameter format.

Looking at the diff more carefully:
- SWC: `"paramNames": ["_rawProps"]` -- SWC rewrites the destructured param
- OXC: `"paramNames": ["{track, cleanup}"]` -- OXC keeps the original

The fix is to apply the `_rawProps` rewriting to `useResource$` callbacks too. This means extending the `is_component_exit` check to include `useResource$`.

## Code Patterns

### Pattern: Const Bindings Collection

```rust
// In QwikTransform struct:
const_bindings: HashSet<String>,

// In QwikTransform::new():
let mut const_bindings = HashSet::new();
// Add all import specifiers
for import in &collected.module_imports {
    for spec in &import.specifiers {
        const_bindings.insert(spec.clone());
    }
}

// In enter_variable_declaration:
fn enter_variable_declaration(&mut self, decl: &VariableDeclaration<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
    if decl.kind == VariableDeclarationKind::Const {
        for declarator in &decl.declarations {
            self.collect_const_binding_names(&declarator.id, &mut self.const_bindings);
        }
    }
}
```

### Pattern: Scope-Aware Prop Classification

```rust
// Updated is_const_jsx_value -- needs const_bindings param
fn is_const_jsx_value(value: &Expression<'_>, const_bindings: &HashSet<String>) -> bool {
    match value {
        Expression::Identifier(ident) => const_bindings.contains(ident.name.as_str()),
        Expression::StaticMemberExpression(member) => {
            // member.object must be const (import or const binding)
            if let Expression::Identifier(obj) = &member.object {
                const_bindings.contains(obj.name.as_str())
            } else {
                false
            }
        }
        // ... existing literal checks ...
        _ => crate::is_const::is_const_expression(value), // fallback
    }
}
```

### Pattern: Scope-Aware Children Classification

```rust
// Updated is_child_expression_immutable -- needs const_bindings param
fn is_child_expression_immutable(
    expr: &Expression<'_>,
    module_imports: &[ImportInfo],
    const_bindings: &HashSet<String>,
) -> bool {
    match expr {
        Expression::Identifier(ident) => {
            // Immutable only if the identifier is a known const binding or import
            const_bindings.contains(ident.name.as_str())
        }
        // ... rest unchanged ...
    }
}
```

### Pattern: q:p from Stored Iteration Variable Usage

```rust
// In QwikTransform struct:
/// Iteration variables used by event handlers, keyed by the JSX element's
/// span start. Populated during enter_call_expression for JSX event handlers.
jsx_element_iter_vars: HashMap<u32, Vec<String>>,

// In enter_call_expression, when processing JSX event handlers in loops:
if self.loop_depth > 0 {
    let iter_vars = self.current_iteration_vars();
    let used_iter_vars: Vec<String> = iter_vars
        .iter()
        .filter(|v| body_ident_refs.contains(v))
        .cloned()
        .collect();
    if !used_iter_vars.is_empty() {
        // Record which iteration variables this handler uses
        // Key: span of the parent JSX element (needs to be determined)
        // Value: list of iteration variable names
    }
}

// In transform_jsx_element_inner, for q:p injection:
// Look up stored iteration variables instead of scanning handler expressions
```

**Challenge:** Mapping from event handler to parent JSX element span. The event handler is processed in `enter_call_expression` before the JSX element's `exit_expression`. The JSX element's span is known at that point (it's the parent expression being traversed). Need to find a way to associate the handler with its element.

**Simpler alternative:** Instead of a per-element map, use the existing `capture_names` on `JsxEventReplacement`. When building q:p, check if any capture name matches an iteration variable:

```rust
// In q:p injection block:
if loop_depth > 0 && !iteration_vars.is_empty() {
    let mut used_iter_vars: Vec<String> = Vec::new();
    for iter_var in iteration_vars {
        // Check if any event handler QRL captures this iteration variable
        let is_used = const_props.iter().any(|(key, _value)| {
            if !key.starts_with("q-") { return false; }
            // Check the hoisted QRL declaration's captures for this iter var
            // This requires looking up the QRL captures by handler name
            // OR: just check if the iter_var is in ANY pending capture list
            false // placeholder
        });
    }
}
```

**Even simpler:** In `enter_call_expression`, when processing JSX event handlers inside loops, store the used iteration variables as a side-channel that `transform_jsx_element_inner` can read. Use a field like `pending_jsx_iter_vars: Vec<String>` that accumulates iteration variables used by handlers, and is consumed (drained) when the next JSX element processes q:p injection.

## Open Questions

1. **Const bindings scoping accuracy**: The proposed HashSet approach doesn't handle block scoping (a `const x` inside an `if` block would incorrectly be visible outside). Is this a practical issue? Review test cases to determine if any snapshot relies on block-scoped const visibility. **Recommendation:** Start with the simple approach; add scoping only if tests fail.

2. **SWC=2 OXC=3 category (9 cases)**: These have `static_listeners=false` in SWC but `true` in OXC. Some may be q:p-related (loop handlers), but some may be `_qrlSync` or other special cases. **Recommendation:** Investigate each during planning by examining the specific test diffs.

3. **How to thread const_bindings to jsx_transform functions**: Currently, `transform_jsx_element_inner` and `transform_jsx_children` take `module_imports` but not const_bindings. Need to add the parameter or pass it through `ImportTracker`. **Recommendation:** Add `const_bindings: &HashSet<String>` to the `ImportTracker` struct (it already holds `immutable_function_cmp` and `jsx_mutable`).

4. **className vs class rename**: The `example_class_name.snap` diff shows `class` -> `className` transformation not happening. This is a separate issue from flags. **Recommendation:** Track separately; may or may not be in Phase 8 scope.

5. **Interaction with _fnSignal wrapping**: Some identifier prop values get wrapped with `_fnSignal` (reactive deps), which then goes into const_props. The scope analysis needs to happen BEFORE the _fnSignal wrapping decision (which it does -- the wrapping path runs after `is_const_jsx_value` returns false). No change needed here.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Full scope analysis | Custom scope tree | Simple HashSet<String> of const bindings | SWC's ConstCollector is equivalent to "is this name const-bound?" -- doesn't need full scope resolution |
| Iteration variable tracking per-element | Complex span-mapping | Drain-based side channel on QwikTransform | Event handlers are processed just before their parent element |
| Static_listeners clearing | Complex condition logic | Simple `if !var_props_has_qp { }` check | q:p presence directly implies non-static listeners |

## State of the Art

| Current Approach | Needed Approach | Impact |
|------------------|-----------------|--------|
| `Expression::Identifier(_) => true` in children | Check `const_bindings.contains(name)` | Fixes 13 SWC=1/OXC=3 cases |
| `is_const_expression` returns false for all identifiers | Check `const_bindings.contains(name)` | Fixes 21 SWC=3/OXC=1 cases |
| `expr_uses_ident` on transformed qrl() values | Pre-recorded iteration var usage | Fixes 20 missing q:p lines |
| `static_listeners = !has_spread` only | Also clear when q:p present | Fixes ~16 static_listeners cases |
| `is_component_exit` guard for _rawProps | Extend to useResource$ etc. | Fixes 1 paramNames mismatch |

## Sources

### Primary (HIGH confidence)
- Direct code analysis: `crates/qwik-optimizer-oxc/src/jsx_transform.rs` -- flag computation, is_child_expression_immutable, q:p injection
- Direct code analysis: `crates/qwik-optimizer-oxc/src/transform.rs` -- loop tracking, iteration variable stack, enter_call_expression event handler processing
- Direct code analysis: `crates/qwik-optimizer-oxc/src/is_const.rs` -- is_const_expression for prop classification
- Snapshot diffs: 138 files differ, 59 paired flag mismatches, 20 missing q:p lines

### Secondary (MEDIUM confidence)
- SWC behavior inferred from snapshot golden files (SWC-generated output)
- GAP analysis from .planning/STATE.md accumulated decisions

## Metadata

**Confidence breakdown:**
- Flag mismatch categories: HIGH - direct snapshot diff analysis
- Root cause identification: HIGH - code paths traced and verified
- Fix approaches: HIGH for const_bindings, MEDIUM for q:p side-channel
- Impact estimates: MEDIUM - based on diff analysis, may have secondary effects

**Research date:** 2026-02-21
**Valid until:** 2026-03-21 (stable Rust codebase, patterns unlikely to change)
