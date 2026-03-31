# Phase 2: Metadata - Research

**Researched:** 2026-02-20
**Domain:** Segment metadata parity (paramNames extraction, path field handling)
**Confidence:** HIGH

## Summary

Phase 2 addresses two metadata issues in the OXC optimizer's segment output: missing `paramNames` arrays and incorrect `path` field handling for Windows paths. Both are well-understood, localized changes with clear reference implementations in the SWC source code.

The `paramNames` issue (META-01) is the primary workload: OXC currently only populates `param_names` during props destructuring (one special case), while SWC extracts parameter names from every arrow function and function expression passed to `$()` calls. This affects ~60 snapshots containing `paramNames` in their golden reference, with ~76 individual diff lines. The SWC implementation (`extract_param_names` at transform.rs:2311-2399) handles simple identifiers, rest params, object destructuring patterns, and array destructuring patterns recursively. Verified that JSX event handler segments (ctxKind: "eventHandler") ALSO have paramNames in golden snapshots -- at least 22 event handler segments across multiple test files include paramNames, so the extraction must cover both regular `$()` calls AND JSX event attribute lambdas.

The `path` field issue (META-02) is much narrower: the OXC `rel_dir` function doesn't normalize Windows backslash separators to forward slashes. SWC uses Rust's `Path` + `to_slash_lossy()` (from the `path-slash` crate) which normalizes separators. This affects only the `support_windows_paths` test case. The `canonicalFilename` field itself is correctly constructed -- the remaining canonicalFilename diffs are from segment ordering issues (Phase 3) and multi-file input missing segments (Phase 3), not from canonicalFilename construction logic.

**Primary recommendation:** Implement `extract_param_names` as a new function in transform.rs that walks OXC `BindingPattern` AST nodes, then call it from both `record_segment` and `create_jsx_event_segments_recursive`. Fix `rel_dir` to normalize backslashes.

## Standard Stack

No new dependencies needed. Both fixes use existing OXC AST types.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| oxc | 0.113 | AST types (BindingPattern, FormalParameters) | Already in use |

### Supporting
No additional libraries needed.

### Alternatives Considered
None -- this is pure internal logic using existing AST types.

## Architecture Patterns

### Where to Place the `extract_param_names` Function

**Location:** `transform.rs` as a standalone function (not a method, since it doesn't need `&self`).

The SWC implementation is a static method `Self::extract_param_names(&expr)` that takes the expression argument to `$()` and returns `Option<Vec<Atom>>`. The OXC equivalent needs two entry points:

1. **For regular `$()` calls:** Takes `&Argument` (the first arg to the call expression)
2. **For JSX event handlers:** Takes the `&JSXAttributeValue` or the expression extracted from the JSX attribute

Both ultimately need to extract params from `FormalParameters`, so the core logic should operate on `&FormalParameters` with wrapper functions for each entry point.

**Recommended approach:**

```
extract_params_from_formal_parameters(&FormalParameters) -> Vec<String>  // core logic
extract_param_names_from_argument(&Argument) -> Vec<String>              // for $() calls
extract_param_names_from_jsx_value(&JSXAttributeValue) -> Vec<String>    // for JSX events
```

### Pattern Conversion: BindingPattern to String

The SWC `pat_to_string` function converts AST patterns to human-readable strings. The OXC equivalent must produce identical output:

| AST Pattern | SWC Output | OXC AST Type |
|-------------|------------|--------------|
| Simple identifier `foo` | `"foo"` | `BindingPattern::BindingIdentifier` |
| Rest param `...args` | `"...args"` | `FormalParameters.rest` (BindingRestElement) |
| Object destructure `{a, b}` | `"{a, b}"` | `BindingPattern::ObjectPattern` |
| Array destructure `[x, y]` | `"[x, y]"` | `BindingPattern::ArrayPattern` |
| Nested `{a: {b}}` | `"{a: {b}}"` | Recursive ObjectPattern |
| Key-value `{key: val}` | `"{key: val}"` | ObjectPattern with `BindingProperty` |
| Assign shorthand `{a}` | `"{a}"` | `BindingProperty` where `shorthand == true` |
| Empty array slot `[,x]` | `"[, x]"` | `ArrayPattern` with `None` element |
| Rest in object `{a, ...r}` | `"{a}"` (rest skipped) | SWC skips `ObjectPatProp::Rest` |

### Call Sites

`extract_param_names` must be called in three places:

1. **`record_segment`** (line ~614 in transform.rs): After creating the `SegmentData`, extract param names from `call.arguments.first()` and set `segment.param_names`. This covers all regular `$()`, `component$()`, `useTask$()`, etc.

2. **`create_jsx_event_segments_recursive`** (line ~757 in transform.rs): When scanning JSX attributes for `$`-suffixed event handlers, the attribute value expression is available. Extract param names from the arrow/function expression in the attribute value and pass them to `record_jsx_event_segment`. This covers `onClick$={(ev) => ...}`, `onInput$={(ev, el) => ...}`, etc.

3. **`record_jsx_event_segment`** (line ~689): Accept param names as a parameter (passed from the caller in `create_jsx_event_segments_recursive`).

**Important interaction with props destructuring:** The existing props destructuring code (line 1660) sets `seg.param_names = vec![info.raw_props_name.clone()]` AFTER the segment is created. This overwrites whatever `extract_param_names` returns. This is correct behavior -- for component$ with destructured props, the param should be the synthetic `_rawProps` name, not the original destructured pattern. The props destructuring override must remain.

### Path Field Fix

**Location:** `lib.rs` function `rel_dir` (line 318).

**Current code:**
```rust
fn rel_dir(path: &str) -> String {
    let last_sep = path.rfind(|c: char| c == '/' || c == '\\');
    if let Some(pos) = last_sep {
        path[..pos].to_string()
    } else {
        String::new()
    }
}
```

**Problem:** Returns the raw path with backslashes preserved. For input `components\\apps\\apps.tsx`, the `rfind` correctly finds the last `\`, and `path[..pos]` extracts `components\\apps\\`, but backslashes are not converted to forward slashes.

**SWC approach:** Uses `path_data.rel_dir` which is built from Rust `Path` operations + `to_slash_lossy()` from the `path-slash` crate. The `to_slash_lossy()` call normalizes all `\` to `/`.

**Corrected code:**
```rust
fn rel_dir(path: &str) -> String {
    let last_sep = path.rfind(|c: char| c == '/' || c == '\\');
    if let Some(pos) = last_sep {
        let dir = &path[..pos];
        dir.replace('\\', "/")
    } else {
        String::new()
    }
}
```

### Anti-Patterns to Avoid
- **Do NOT add path-slash dependency** -- a simple `replace('\\', '/')` is sufficient and avoids adding a new dependency for one function.
- **Do NOT modify canonicalFilename construction** -- the canonicalFilename diffs in the snapshots are from segment ordering (Phase 3 blocker), not from canonicalFilename logic itself.
- **Do NOT change how `segment_data_to_analysis` constructs `path`** from `rel_dir(origin_path)` -- this is correct. Only `rel_dir` itself needs fixing.
- **Do NOT try to fix multi-file input path diffs** -- the `relative_paths` and `example_missing_custom_inlined_functions` path diffs are caused by missing segments (Phase 3 issue), not path field logic.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Pattern-to-string conversion | Ad-hoc string building | Recursive function matching SWC's `pat_to_string` exactly | SWC has specific formatting rules (curly braces, brackets, commas with spaces, rest skipping in objects) |

## Common Pitfalls

### Pitfall 1: Props Destructuring Override
**What goes wrong:** If `extract_param_names` is called but the props destructuring code also runs, you could end up with the wrong param names for component$ segments.
**Why it happens:** The props destructuring code (line 1660) overwrites `seg.param_names` after initial extraction.
**How to avoid:** Set param_names in `record_segment` initially from `extract_param_names`, then let props destructuring override it later (current flow works correctly -- overwrite is intentional).
**Warning signs:** component$ segments showing `"{count, rest}"` instead of `"_rawProps"` in paramNames.

### Pitfall 2: JSX Event Handler Param Names (CONFIRMED REQUIRED)
**What goes wrong:** JSX event handlers like `onClick$={(ev) => ...}` should include `["ev"]` in paramNames, but `record_jsx_event_segment` doesn't have access to the expression.
**Why it happens:** `record_jsx_event_segment` is called from `create_jsx_event_segments_recursive` which pre-scans JSX elements but doesn't extract function params.
**Confirmed:** At least 22 event handler segments across multiple golden snapshots (e.g., `should_extract_single_qrl`, `should_transform_multiple_event_handlers`, `example_component_with_event_listeners_inside_loop`) include paramNames like `["_", "_", "row"]`, `["ev"]`, `["btn"]`.
**How to avoid:** Extract param names from the JSX attribute value expression during `create_jsx_event_segments_recursive` and pass them to `record_jsx_event_segment`. The attribute value is a `JSXAttributeValue::ExpressionContainer` containing an `ArrowFunctionExpression` or `FunctionExpression`.
**Warning signs:** Event handler segments missing paramNames that SWC includes.

### Pitfall 3: Object Pattern Rest Properties Skipped
**What goes wrong:** SWC's `pat_to_string` explicitly skips `ObjectPatProp::Rest` entries in object patterns. If OXC includes them, the output won't match.
**Why it happens:** SWC code at line 2355: `ast::ObjectPatProp::Rest(_) => {}` -- a no-op.
**How to avoid:** Match SWC behavior exactly: skip rest properties in object destructuring patterns. Note: top-level rest parameters (`...args`) ARE included (handled by `Pat::Rest` / `FormalParameters.rest`), but rest inside object patterns `{a, ...r}` are not.
**Warning signs:** paramNames containing `"{a, ...rest}"` when SWC shows `"{a}"`.

### Pitfall 4: Windows Path Double-Escaping in Tests
**What goes wrong:** The test input uses `"components\\\\apps\\\\apps.tsx"` which in Rust is the literal string `components\\apps\\apps.tsx` (each `\\\\` becomes `\\` literal).
**Why it happens:** The test string has double-escaped backslashes representing literal double-backslash sequences.
**How to avoid:** The `rel_dir` fix should handle all `\` characters regardless of how many there are. The `replace('\\', '/')` approach handles this correctly since it replaces each individual backslash.
**Warning signs:** Path output containing `\\` instead of `/`.

### Pitfall 5: OXC BindingProperty vs SWC ObjectPatProp
**What goes wrong:** OXC's `BindingProperty` has a `shorthand` boolean flag, while SWC uses separate enum variants (`Assign` for shorthand, `KeyValue` for non-shorthand).
**Why it happens:** Different AST representations between OXC and SWC.
**How to avoid:** Check `prop.shorthand` in OXC: if true, use the key name directly (equivalent to SWC's `Assign`); if false, format as `"key: value"` (equivalent to SWC's `KeyValue`).
**Warning signs:** Shorthand properties appearing as `"{a: a}"` instead of `"{a}"`.

## Code Examples

### SWC extract_param_names Reference (transform.rs:2311-2399)

```rust
// SWC's implementation -- the canonical reference
fn extract_param_names(expr: &ast::Expr) -> Option<Vec<Atom>> {
    fn pat_to_string(pat: &ast::Pat) -> Option<Atom> {
        match pat {
            ast::Pat::Ident(ident) => Some(ident.id.sym.clone()),
            ast::Pat::Rest(rest) => {
                pat_to_string(&rest.arg).map(|name| Atom::from(format!("...{}", name)))
            }
            ast::Pat::Array(array) => {
                let mut parts = Vec::new();
                for elem in &array.elems {
                    match elem {
                        Some(pat) => {
                            if let Some(name) = pat_to_string(pat) {
                                parts.push(name.to_string());
                            }
                        }
                        None => parts.push("".to_string()),
                    }
                }
                if parts.is_empty() { None }
                else { Some(Atom::from(format!("[{}]", parts.join(", ")))) }
            }
            ast::Pat::Object(obj) => {
                let mut parts = Vec::new();
                for prop in &obj.props {
                    match prop {
                        ast::ObjectPatProp::KeyValue(kv) => {
                            let key = match &kv.key {
                                ast::PropName::Ident(ident) => ident.sym.to_string(),
                                ast::PropName::Str(str) => str.value.to_string(),
                                ast::PropName::Num(num) => num.value.to_string(),
                                ast::PropName::BigInt(bigint) => bigint.value.to_string(),
                                ast::PropName::Computed(_) => continue,
                            };
                            if let Some(value) = pat_to_string(&kv.value) {
                                parts.push(format!("{}: {}", key, value));
                            }
                        }
                        ast::ObjectPatProp::Assign(assign) => {
                            parts.push(assign.key.sym.to_string());
                        }
                        ast::ObjectPatProp::Rest(_) => {
                            // Skip rest properties in object patterns
                        }
                    }
                }
                if parts.is_empty() { None }
                else { Some(Atom::from(format!("{{{}}}", parts.join(", ")))) }
            }
            _ => None,
        }
    }

    match expr {
        ast::Expr::Arrow(arrow) => {
            let mut names = Vec::with_capacity(arrow.params.len());
            for param in &arrow.params {
                if let Some(name) = pat_to_string(param) { names.push(name); }
            }
            if names.is_empty() { None } else { Some(names) }
        }
        ast::Expr::Fn(fn_expr) => {
            let mut names = Vec::with_capacity(fn_expr.function.params.len());
            for param in &fn_expr.function.params {
                if let Some(name) = pat_to_string(&param.pat) { names.push(name); }
            }
            if names.is_empty() { None } else { Some(names) }
        }
        _ => None,
    }
}
```

### OXC Equivalent (to be implemented in transform.rs)

```rust
/// Convert a BindingPattern to a human-readable string for paramNames metadata.
/// Matches SWC's `pat_to_string` (transform.rs:2312-2368).
fn binding_pattern_to_string(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            Some(ident.name.as_str().to_string())
        }
        BindingPattern::ObjectPattern(obj) => {
            let mut parts = Vec::new();
            for prop in &obj.properties {
                if prop.shorthand {
                    // Shorthand {a} -- equivalent to SWC's ObjectPatProp::Assign
                    if let BindingPattern::BindingIdentifier(ident) = &prop.value {
                        parts.push(ident.name.as_str().to_string());
                    }
                } else {
                    // KeyValue {key: value} -- equivalent to SWC's ObjectPatProp::KeyValue
                    let key_str = match &prop.key {
                        PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                        PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
                        PropertyKey::NumericLiteral(n) => n.value.to_string(),
                        PropertyKey::BigIntLiteral(b) => b.raw.as_str().to_string(),
                        _ => continue, // Computed keys: skip
                    };
                    if let Some(value) = binding_pattern_to_string(&prop.value) {
                        parts.push(format!("{}: {}", key_str, value));
                    }
                }
            }
            // Skip rest properties in object patterns (SWC behavior)
            if parts.is_empty() { None }
            else { Some(format!("{{{}}}", parts.join(", "))) }
        }
        BindingPattern::ArrayPattern(arr) => {
            let mut parts = Vec::new();
            for elem in &arr.elements {
                match elem {
                    Some(pat) => {
                        if let Some(name) = binding_pattern_to_string(pat) {
                            parts.push(name);
                        }
                    }
                    None => parts.push(String::new()),
                }
            }
            if parts.is_empty() { None }
            else { Some(format!("[{}]", parts.join(", "))) }
        }
        BindingPattern::AssignmentPattern(assign) => {
            // Assignment pattern with default: use the left-hand side
            binding_pattern_to_string(&assign.left)
        }
    }
}

/// Extract parameter names from FormalParameters.
/// Matches SWC's `extract_param_names` inner logic (transform.rs:2370-2399).
fn extract_param_names_from_params(params: &FormalParameters<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for param in &params.items {
        if let Some(name) = binding_pattern_to_string(&param.pattern) {
            names.push(name);
        }
    }
    // Handle rest parameter: ...args
    if let Some(rest) = &params.rest {
        if let Some(name) = binding_pattern_to_string(&rest.argument) {
            names.push(format!("...{}", name));
        }
    }
    names
}

/// Extract parameter names from a $() call argument.
fn extract_param_names_from_argument(arg: &Argument<'_>) -> Vec<String> {
    match arg {
        Argument::ArrowFunctionExpression(arrow) => {
            extract_param_names_from_params(&arrow.params)
        }
        Argument::FunctionExpression(func) => {
            extract_param_names_from_params(&func.function.params)
        }
        _ => Vec::new(),
    }
}

/// Extract parameter names from a JSX attribute value (event handler).
fn extract_param_names_from_jsx_value(value: &JSXAttributeValue<'_>) -> Vec<String> {
    if let JSXAttributeValue::ExpressionContainer(container) = value {
        match &container.expression {
            JSXExpression::ArrowFunctionExpression(arrow) => {
                extract_param_names_from_params(&arrow.params)
            }
            JSXExpression::FunctionExpression(func) => {
                extract_param_names_from_params(&func.function.params)
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    }
}
```

### Integration Points

**In `record_segment` (after creating SegmentData):**
```rust
let segment = SegmentData {
    // ... existing fields ...
    param_names: if let Some(first_arg) = call.arguments.first() {
        extract_param_names_from_argument(first_arg)
    } else {
        vec![]
    },
    // ... rest of fields ...
};
```

**In `create_jsx_event_segments_recursive` (when scanning attributes):**
```rust
// Extract param names from the attribute value expression
let param_names = if let Some(value) = &attr.value {
    extract_param_names_from_jsx_value(value)
} else {
    vec![]
};
// Pass to record_jsx_event_segment
self.record_jsx_event_segment(span, &ctx_name, param_names);
```

**In `record_jsx_event_segment` (updated signature):**
```rust
pub(crate) fn record_jsx_event_segment(
    &mut self,
    span: (u32, u32),
    ctx_name: &str,
    param_names: Vec<String>,  // NEW parameter
) -> SegmentData {
    // ... existing code ...
    let segment = SegmentData {
        // ... existing fields ...
        param_names,  // Use the passed-in param names
        // ... rest of fields ...
    };
    // ...
}
```

### rel_dir Fix (lib.rs)

```rust
/// Extract the directory part of a path, normalizing separators to forward slashes.
fn rel_dir(path: &str) -> String {
    let last_sep = path.rfind(|c: char| c == '/' || c == '\\');
    if let Some(pos) = last_sep {
        path[..pos].replace('\\', "/")
    } else {
        String::new()
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| param_names only set for props destructuring | param_names extracted from every $() call and JSX event handler | This phase | Adds paramNames to ~60 snapshots |
| rel_dir preserves backslashes | rel_dir normalizes to forward slashes | This phase | Fixes 1 snapshot (windows paths) |

## Open Questions

1. **Multi-file input path diffs (DEFERRED)**
   - What we know: The `relative_paths` and `example_missing_custom_inlined_functions` tests show path diffs, but these are caused by missing segments from multi-file inputs (Phase 3), not by path field logic.
   - Recommendation: Ignore these diffs in Phase 2. They will resolve when Phase 3 fixes missing segments from multi-file inputs.

2. **FunctionExpression in $() calls**
   - What we know: SWC handles both `ast::Expr::Arrow` and `ast::Expr::Fn` in `extract_param_names`. OXC test cases use arrow functions almost exclusively.
   - Recommendation: Implement both branches for correctness, even if only arrow functions appear in current tests. The `should_transform_component_with_normal_function` test exists and likely uses a function expression.

## Sources

### Primary (HIGH confidence)
- SWC transform.rs `extract_param_names` (lines 2311-2399) -- direct source code inspection
- SWC transform.rs `SegmentData` construction (lines 451-463, 719-731) -- path field assignment from `path_data.rel_dir`
- SWC parse.rs `PathData` struct and `parse_path` function (lines 716-755) -- rel_dir semantics with `to_slash_lossy()`
- OXC transform.rs `record_segment` (lines 598-633) -- current param_names: vec![] always
- OXC transform.rs `record_jsx_event_segment` (lines 689-748) -- current param_names: vec![] always
- OXC transform.rs `create_jsx_event_segments_recursive` (line ~757) -- where JSX attr values are accessible
- OXC lib.rs `segment_data_to_analysis` (lines 327-362) -- path/canonicalFilename construction
- OXC lib.rs `rel_dir` (lines 318-325) -- current path extraction without backslash normalization
- Golden snapshot analysis: 60 snapshots with paramNames, 22+ eventHandler segments with paramNames, 1 snapshot with path normalization issue
- Snapshot diff: SWC `"path": "components/apps"` vs OXC `"path": "components\\\\apps\\"` in support_windows_paths

### Secondary (MEDIUM confidence)
- Snapshot diff analysis showing ~76 paramNames diff lines across the test suite

### Tertiary (LOW confidence)
- None -- all findings verified against source code

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, purely internal logic
- Architecture: HIGH -- clear 1:1 mapping from SWC reference to OXC implementation, verified against both codebases
- Pitfalls: HIGH -- all pitfalls verified against SWC source code and golden snapshot analysis

**Research date:** 2026-02-20
**Valid until:** Indefinite (both codebases are under our control, SWC reference is stable golden output)
