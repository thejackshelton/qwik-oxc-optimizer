# Phase 5: JSX Keys & Flags - Research

**Researched:** 2026-02-20
**Domain:** Qwik JSX transform — key generation and immutability flags
**Confidence:** HIGH

## Summary

Phase 5 addresses two distinct but related issues in the Qwik JSX transform output: (1) JSX key generation that does not match SWC's key-vs-null decisions, and (2) immutability flags that are always `3` instead of the correct `0`, `1`, or `2` values SWC produces.

The **key generation** problem has two sub-issues: (a) the key prefix is hardcoded to `"u6"` instead of being computed from `base64(file_hash)[0..2]`, and (b) every element receives a generated key when SWC only emits keys for "root" JSX elements and component (function) tags, using `null` for nested native elements.

The **immutability flags** problem stems from the OXC code using a simplistic `children_count > 1 ? 1 : 3` formula instead of tracking per-element mutability state (`static_listeners` and `static_subtree`) like SWC does.

**Primary recommendation:** Implement SWC's `root_jsx_mode` / `should_emit_key` / `jsx_mutable` state tracking in the OXC transform, computing proper flag bits from actual prop/child mutability analysis.

## Architecture Patterns

### Pattern 1: SWC Key Generation Algorithm

**What:** SWC generates JSX keys based on `should_emit_key = is_fn || root_jsx_mode`, where `is_fn` is true for component (uppercase) tags and `root_jsx_mode` is true for the first JSX element in each function body, reset to false after.

**How it works:**

```
// SWC transform.rs lines 876-898
let should_emit_key = is_fn || self.root_jsx_mode;
let prev = self.root_jsx_mode;
self.root_jsx_mode = false;

// ... process props and children ...

let key = if node.args.len() == 1 {
    // User-provided key from JSX `key` prop
    node.args.remove(0)
} else if should_emit_key {
    // Generate auto-key: "XX_N" where XX = base64(file_hash)[0..2], N = counter
    let new_key = format!("{}_{}", &base64(self.file_hash)[0..2], self.jsx_key_counter);
    self.jsx_key_counter += 1;
    // ... string literal ...
} else {
    // No key for nested native elements
    get_null_arg()
};
```

**Key prefix computation:**
```
// file_hash = DefaultHasher(scope? bytes, rel_path bytes).finish()
// Key prefix = base64url(file_hash.to_le_bytes())[0..2] with -/_ replaced by 0
```

For test files with rel_path "test.tsx" and no scope, this produces `"u6"`.

**When keys are emitted vs null:**
- `is_fn = true` (component tag like `<Foo>`, `<App>`, `<_Fragment>`) -> ALWAYS emit key
- `root_jsx_mode = true` (first JSX element in a function body) -> emit key
- Neither -> emit `null`

**`root_jsx_mode` lifecycle:**
- Set to `true` at entry of every `fold_fn_expr`, `fold_arrow_expr`, `fold_for_stmt`, `fold_for_in_stmt`, `fold_for_of_stmt`, `fold_while_stmt`, `fold_do_while_stmt`, `fold_if_stmt`, `fold_block_stmt`, `fold_return_stmt`
- Set to `false` after the first JSX element is processed in `handle_jsx` (line 878)
- Restored to previous value after each fold method returns

**Source:** SWC `crates/swc-optimizer/core/src/transform.rs` lines 163-164 (init), 269 (init true), 850-919 (handle_jsx), 2672-2698 (fold_fn_expr), 2718-2780 (fold_arrow_expr), 2788-2924 (fold_for/while/if/block/return stmts)

### Pattern 2: SWC Immutability Flags (Bitfield)

**What:** SWC tracks two boolean flags per JSX element during prop/child processing, encoding them as a 2-bit field:

```
Bit 0 (value 1): static_listeners -- all event handlers are const (QRL calls, not raw functions)
Bit 1 (value 2): static_subtree  -- all children and the subtree are immutable

Flag values:
  0 = spread props (no static guarantees)
  1 = static listeners only (events const, but children/props are mutable)
  2 = static subtree only (children immutable, but listeners mutable) -- rare
  3 = fully static (both listeners and subtree immutable)
```

**How flags are computed:**

```
// SWC transform.rs lines 1570-1571
let mut static_listeners = !has_spread_props;  // spread kills static_listeners
let mut static_subtree = !has_spread_props;     // spread kills static_subtree

// During prop processing:
// - var_props existence sets jsx_mutable (line 1469)
// - Non-const event handler sets static_listeners = false (lines 1729, 1804)
// - Children processing: jsx_mutable flag tracked per-child
//   - If ANY child sets jsx_mutable = true, static_subtree = false (line 1657)
//   - jsx_mutable is set by: non-JSX call expressions, tagged templates,
//     non-const expressions, component tags not in immutable_function_cmp

// After all props/children processed:
let mut flags = 0;
if static_listeners { flags |= 1 << 0; }  // bit 0
if static_subtree { flags |= 1 << 1; }    // bit 1
```

**What makes things mutable (sets jsx_mutable = true, breaks static_subtree):**
1. **var_props** -- any non-empty var_props object (line 1469)
2. **Component tags** -- non-native JSX tags that aren't in `immutable_function_cmp` set (line 866-867)
3. **Non-JSX function calls in children** -- calls not recognized as jsx() functions (lines 2037-2038, 2214-2215)
4. **Tagged template expressions** in children (line 2050)
5. **Non-const, non-wrappable expressions** -- when `convert_to_signal_item` returns is_const=false (line 2228-2229)

**What does NOT make things mutable:**
1. String literals, numbers, booleans, template literals without expressions
2. `_wrapProp()` calls (these are const -- signal wrapping is a const operation)
3. `_fnSignal()` calls (these are const -- derived signals)
4. Const expressions (imports, pure identifiers, etc.)
5. JSX-recognized function calls (jsx(), jsxs(), jsxDEV())
6. Component tags in `immutable_function_cmp` set (Fragment, RenderOnce, Link)

**Source:** SWC `crates/swc-optimizer/core/src/transform.rs` lines 1570-1571 (init), 1931-1937 (flag encoding), 1466-1469 (var_props), 866-867 (component tags), 2037-2042 (call children)

### Pattern 3: SWC `immutable_function_cmp` Set

**What:** A set of component identifiers that are considered "immutable" -- using them as JSX tags does NOT set `jsx_mutable = true`.

**Members (from SWC transform.rs lines 205-232):**
- `Fragment` from `@qwik.dev/core/jsx-runtime` or `@qwik.dev/core/jsx-dev-runtime`
- `Fragment` from `@qwik.dev/core`
- `RenderOnce` from `@qwik.dev/core`
- `Link` from `@qwik.dev/router`
- Any import from a `?jsx` or `.md` source

**Source:** SWC `crates/swc-optimizer/core/src/transform.rs` lines 205-232

### Anti-Patterns to Avoid

- **Blanket flag values:** OXC currently uses `children_count > 1 ? 1 : 3` which is wrong. Flags must be computed from actual mutability analysis.
- **Keys for everything:** OXC generates keys for all elements. SWC only generates keys for "root" elements and component tags.
- **Hardcoded key prefix:** The `"u6"` prefix works for `test.tsx` but would be wrong for any other filename.
- **Global key counter without scope:** OXC shares one counter across all components in the same module, which is actually correct (matches SWC's behavior), but the decision of WHEN to emit vs null is wrong.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Base64 key prefix | Hardcoded `"u6"` | `base64(DefaultHasher(scope?, rel_path).finish()).to_le_bytes())[0..2]` with `-/_` -> `0` | Must vary per file |
| Mutability tracking | Simple heuristic from children_count | Proper `static_listeners` + `static_subtree` booleans | SWC tracks these precisely through prop/child processing |

## Common Pitfalls

### Pitfall 1: root_jsx_mode Scope

**What goes wrong:** Implementing `root_jsx_mode` at the wrong granularity (e.g., only in function bodies, missing if/for/while/block statements).

**Why it happens:** SWC sets `root_jsx_mode = true` at the entry of MANY statement types, not just function bodies. This ensures that the first JSX element inside any `if` branch, `for` loop body, `while` loop body, `return` statement, or block statement also gets a key.

**How to avoid:** In OXC's Traverse API, hook into all statement-level enter/exit handlers that SWC's fold methods handle: functions, arrows, for/for-in/for-of, while, do-while, if, block, return statements.

**Warning signs:** Tests where JSX inside `if` or `for` bodies gets `null` keys instead of generated keys.

### Pitfall 2: jsx_mutable State Leaking Between Siblings

**What goes wrong:** The `jsx_mutable` flag from one child element leaks to affect sibling elements' flag computation.

**Why it happens:** SWC carefully saves/restores `jsx_mutable` around children processing (lines 1640-1641, 1656-1659). If the OXC implementation doesn't do this, a mutable child would incorrectly make the parent's subtree flag wrong.

**How to avoid:** Always save `jsx_mutable` before processing children, check it after, then restore if children were immutable.

### Pitfall 3: var_props Implying Mutability

**What goes wrong:** Flag stays at `3` even when var_props exist.

**Why it happens:** In SWC, the mere existence of non-empty var_props triggers `jsx_mutable = true` (line 1469). OXC's code builds var_props but doesn't track this as a mutability signal.

**How to avoid:** After building var_props, if non-empty, set the mutability flag which will cause `static_subtree = false`.

### Pitfall 4: Component Tags and Mutability

**What goes wrong:** `<Foo>` or `<Cmp>` component tags don't affect the flag properly.

**Why it happens:** SWC marks non-immutable component tags as mutable (line 866-867). `Fragment` is in the immutable set so `<></>` doesn't break immutability, but custom components do.

**How to avoid:** Track the `immutable_function_cmp` set (Fragment, RenderOnce, Link, ?jsx imports). When a JSX element has a component tag not in this set, mark the parent's subtree as mutable.

### Pitfall 5: Key Counter is File-Global, Not Per-Component

**What goes wrong:** Trying to reset the key counter per component/function.

**Why it happens:** The SWC counter is on the transform struct and NEVER reset -- it just increments throughout the entire file. The OXC code already does this correctly (counter on ImportTracker), but it's tempting to think it should reset.

**How to avoid:** Keep the counter global. The important distinction is WHEN keys are emitted (root_jsx_mode || is_fn) vs null, not the counter value.

## Code Examples

### Current OXC Flag Logic (WRONG)

```rust
// jsx_transform.rs lines 1319-1325
let flags = if has_spread {
    0
} else if children_count > 1 {
    1
} else {
    3
};
```

### Correct SWC Flag Logic

```rust
// Flags should be computed like this (pseudo-Rust):
let mut static_listeners = !has_spread;
let mut static_subtree = !has_spread;

// During prop classification:
if !var_props.is_empty() {
    // var_props existence breaks static_subtree
    static_subtree = false;
}

// For each event handler:
if !event_handler_is_const {
    static_listeners = false;
}

// During children processing:
// save mutable flag, process children, check if children set it
// if any child was mutable -> static_subtree = false

let mut flags: u32 = 0;
if static_listeners { flags |= 1; }
if static_subtree { flags |= 2; }
```

### Current OXC Key Logic (WRONG)

```rust
// jsx_transform.rs lines 1328-1342
let key_expr = if let Some(key) = key_value {
    key
} else if children_count > 0 || has_any_visible_prop || !const_props.is_empty() || !var_props.is_empty() {
    let key_str = format!("u6_{}", tracker.jsx_key_counter);
    tracker.jsx_key_counter += 1;
    ctx.ast.expression_string_literal(SPAN, ...)
} else {
    ctx.ast.expression_null_literal(SPAN)
};
```

### Correct SWC Key Logic

```rust
// Key should be computed like this:
let is_fn = tag_is_component_or_fragment; // uppercase first char or in jsx_functions
let should_emit_key = is_fn || root_jsx_mode;

// After entering handle_jsx, reset root_jsx_mode:
// root_jsx_mode = false;

let key_expr = if let Some(key) = user_provided_key {
    key
} else if should_emit_key {
    let prefix = &base64(file_hash)[0..2]; // e.g., "u6" for test.tsx
    let key_str = format!("{}_{}", prefix, key_counter);
    key_counter += 1;
    ctx.ast.expression_string_literal(...)
} else {
    ctx.ast.expression_null_literal(SPAN)
};
```

### File Hash for Key Prefix

```rust
// Compute file_hash at initialization:
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use base64::Engine;

let mut hasher = DefaultHasher::new();
if let Some(scope) = &options.scope {
    hasher.write(scope.as_bytes());
}
hasher.write(rel_path.as_bytes()); // e.g., "test.tsx"
let file_hash = hasher.finish();

let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .encode(file_hash.to_le_bytes());
let key_prefix = encoded[0..2].replace(['-', '_'], "0");
// For "test.tsx" -> "u6"
```

## Detailed Diff Analysis

### Affected Snapshot Categories

From analyzing the full diff output:

1. **Keys: null -> "u6_N"** (~60 snapshots): Native `<div>`, `<span>`, etc. inside parent JSX get `"u6_N"` in OXC but `null` in SWC. These are nested elements where `should_emit_key` would be false.

2. **Flags: 1 -> 3** (~80 snapshots): Elements with var_props or mutable children get `3` in OXC but `1` in SWC. Examples:
   - `globalThing` as child -> SWC `1`, OXC `3` (global identifier makes subtree mutable)
   - `signal.value()` as child -> SWC `1`, OXC `3` (function call makes subtree mutable)
   - `mutable(signal)` as child -> SWC `1`, OXC `3` (function call makes subtree mutable)
   - `signal.value + dep` as child -> SWC `1`, OXC `3` (mixed expression makes subtree mutable)

3. **Fragment flags: 3 -> 1** (~5 snapshots): Fragment with single child that's a `_wrapProp` call gets `3` in OXC but `1` in SWC. This happens when the `_wrapProp` child makes the fragment mutable.

### Other Diffs in Same Snapshots

Many of the 160 differing snapshots also have import ordering (Phase 6) and formatting (non-phase) diffs mixed in. The key/flag changes are specifically in the `_jsxSorted()` / `_jsxSplit()` call arguments (positions 5 and 6 in the 6-argument calls).

## Implementation Strategy

### Requirement JSX-01: Key Generation

1. **Add `file_hash: u64` field** to `QwikTransform` or `ImportTracker`, computed from `DefaultHasher(scope?, filename)` at initialization
2. **Add `jsx_key_prefix: String` field** computed as `base64(file_hash)[0..2]` with `-/_` -> `0` replacement
3. **Add `root_jsx_mode: bool` field** to track whether the next JSX element is the "first" in current scope
4. **Implement root_jsx_mode lifecycle:** set to `true` on entering function/arrow bodies and statement blocks (for/while/if/return/block), set to `false` after first JSX element processes
5. **Update key generation logic:** use `should_emit_key = is_fn || root_jsx_mode` to decide key vs null

### Requirement JSX-02: Immutability Flags

1. **Add `jsx_mutable: bool` field** to track current subtree mutability (or pass through as parameter)
2. **Track `static_listeners` and `static_subtree`** per element during prop/child processing
3. **Set jsx_mutable = true** when:
   - var_props is non-empty
   - Component tag not in immutable set
   - Non-JSX call expressions in children
   - Non-const, non-wrappable expressions in children
4. **Save/restore jsx_mutable** around children processing
5. **Compute flags** as bitfield: `(static_listeners ? 1 : 0) | (static_subtree ? 2 : 0)`
6. **Build immutable_function_cmp set** from imports (Fragment, RenderOnce, Link, ?jsx sources)

### Estimated Complexity

- JSX-01 (keys): Medium. Requires threading `root_jsx_mode` through the OXC traverse hooks, which means adding state to `QwikTransform` and managing save/restore in enter/exit handlers. The file_hash computation is straightforward.

- JSX-02 (flags): Medium-High. Requires threading mutability tracking through the recursive JSX transform functions. The challenge is that `transform_jsx_element_inner` and its helpers need access to parent mutability state, and the state must flow correctly through recursive child processing.

## Open Questions

1. **root_jsx_mode in OXC Traverse vs SWC Fold:**
   - SWC uses `fold_*` methods which naturally wrap enter+process+exit
   - OXC uses `enter_*` / `exit_*` pairs in `Traverse`
   - **Recommendation:** Use `enter_*` to save+set `root_jsx_mode = true`, use `exit_*` to restore previous value. Need to identify exactly which `enter_*` / `exit_*` pairs map to SWC's fold methods.
   - **Status:** HIGH confidence this approach works -- prior phases used similar save/restore patterns on `QwikTransform` state.

2. **jsx_mutable threading through jsx_transform.rs functions:**
   - The `transform_jsx_element_inner` function currently takes `&mut ImportTracker` but no mutability state
   - **Recommendation:** Either add `&mut bool` parameters for jsx_mutable, or add the field to ImportTracker
   - **Status:** MEDIUM confidence. The function signatures are already long. Adding to ImportTracker may be cleaner.

3. **immutable_function_cmp in OXC:**
   - OXC doesn't currently have this set
   - Needs to be built from the collected imports (Fragment, RenderOnce, Link, ?jsx sources)
   - **Recommendation:** Build from `CollectResult.module_imports` during `QwikTransform::new()` or pass as parameter
   - **Status:** HIGH confidence -- the import data is available in `CollectResult`.

## Sources

### Primary (HIGH confidence)
- SWC `crates/swc-optimizer/core/src/transform.rs` -- direct source code analysis
  - Lines 121-122: `jsx_mutable`, `jsx_key_counter` fields
  - Lines 163-183: `file_hash` computation from `DefaultHasher(scope?, rel_path)`
  - Lines 205-232: `immutable_function_cmp` set initialization
  - Lines 850-925: `handle_jsx` -- key generation and flag assembly
  - Lines 1451-1487: `handle_jsx_props_obj` -- var/const classification
  - Lines 1490-1967: `internal_handle_jsx_props_obj` -- `static_listeners`, `static_subtree` tracking
  - Lines 2028-2075: `convert_children` -- child mutability detection
  - Lines 2206-2237: `convert_to_signal_item` -- signal child mutability
  - Lines 2672-2698: `fold_fn_expr` -- `root_jsx_mode` save/set/restore
  - Lines 2718-2780: `fold_arrow_expr` -- `root_jsx_mode` save/set/restore
  - Lines 2788-2944: fold for/while/if/block/return -- `root_jsx_mode` set
  - Lines 3433-3437: `base64()` helper function

- OXC `crates/qwik-optimizer-oxc/src/jsx_transform.rs` -- direct source code analysis
  - Lines 1001-1524: `transform_jsx_element_inner` -- current key/flag logic
  - Lines 1319-1325: current flag computation (WRONG)
  - Lines 1328-1342: current key generation (WRONG)
  - Lines 1527-1589: `transform_jsx_fragment_inner`
  - Lines 1593-1785: `transform_jsx_children`

- OXC `crates/qwik-optimizer-oxc/src/transform.rs` -- direct source code analysis
  - Lines 38-100: `ImportTracker` struct (has `jsx_key_counter` but no mutability tracking)
  - Lines 102-200: `QwikTransform` struct (has `jsx_element_is_native` but no `root_jsx_mode`)

- Snapshot diff analysis -- `git diff` of 160 files between OXC and SWC golden output
  - Key diffs: ~60 snapshots with null-vs-"u6_N" mismatches
  - Flag diffs: ~80 snapshots with 1-vs-3 or other flag mismatches

## Metadata

**Confidence breakdown:**
- Key generation algorithm: HIGH -- direct source code analysis of SWC, clear algorithm
- Flag computation: HIGH -- direct source code analysis, well-documented bitfield
- root_jsx_mode mapping to OXC: MEDIUM -- needs careful mapping of SWC fold methods to OXC traverse hooks
- immutable_function_cmp: HIGH -- simple set lookup from imports

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (stable -- SWC reference code is frozen, not changing)
