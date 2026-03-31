# Phase 3: Bugs & Correctness - Research

**Researched:** 2026-02-20
**Domain:** AST transformation correctness, TypeScript stripping, OXC codegen, capture analysis
**Confidence:** HIGH (based on direct source code analysis + snapshot diff examination)

## Summary

Phase 3 addresses six correctness bugs (BUG-01 through BUG-06) in the OXC Qwik optimizer. Research was conducted by examining the actual snapshot diffs (160 files, ~3000 insertions / ~8200 deletions), the OXC source code (`crates/qwik-optimizer-oxc/src/`), the SWC reference implementation (`crates/swc-optimizer/core/src/`), and the OXC framework documentation.

The six bugs range from straightforward (BUG-02: component options dropped -- a 5-line fix) to structural (BUG-01: TypeScript stripping requires enabling a new crate feature, BUG-05: test fixture replacement). The bugs are largely independent and can be fixed in parallel or in any order, though BUG-01 (TS stripping) affects the most snapshots across the board.

**Primary recommendation:** Fix bugs in this order: BUG-02 (trivial, 10 snapshots), BUG-06 (comments, ~5 snapshots), BUG-01 (TS stripping, ~20 snapshots), BUG-03 (captures, ~50 snapshots -- many are symptoms of Phase 4 missing features), BUG-04 (missing segments, ~30 snapshots -- includes test fixture fix), BUG-05 (qwik_router_inline, 1 snapshot -- test fixture problem). Importantly, many BUG-03 capture diffs are caused by missing Phase 4 features (_wrapProp, _fnSignal, props destructuring), NOT by capture analysis bugs.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `oxc` | 0.113 | Parser, AST, traverse, codegen | Already in use, umbrella crate |
| `oxc` (with `"transformer"` feature) | 0.113 | TypeScript type stripping | Needed for BUG-01 -- provides `oxc::transformer::Transformer` |
| `oxc_traverse` | 0.113 | Traverse trait for AST mutation | Already in use |

### Supporting (already present, no new deps needed)
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `insta` | 1.x | Snapshot testing | Verification after each fix |
| `serde_json` | 1.x | JSON output | Segment metadata |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `oxc` `"transformer"` feature for TS | Manual AST stripping in traverse | Manual stripping is error-prone (must handle type annotations, interfaces, type aliases, type imports, enums, etc.) -- oxc_transformer handles all edge cases correctly |
| `oxc_isolated_declarations` for TS | Only generates `.d.ts` -- does NOT strip types from JS output | Wrong tool for this job |

**Installation (Cargo.toml change for BUG-01):**
```toml
oxc = { version = "0.113", features = [
    "codegen",
    "semantic",
    "serialize",
    "ast_visit",
    "transformer",  # NEW: enables oxc_transformer for TS stripping
] }
```

## Architecture Patterns

### BUG-01: TypeScript Stripping via oxc_transformer

**What:** SWC uses `typescript::strip(Default::default(), top_level_mark)` (SWC parse.rs line 244) to remove all TypeScript type annotations from the AST before the Qwik transform pass runs. OXC currently does NOT strip types -- `transpile_ts` only affects output file extension.

**Approach:** Add the `"transformer"` feature to the `oxc` dependency and run `oxc::transformer::Transformer` with TypeScript-only config BEFORE the Qwik transform traverse. This mirrors SWC's pipeline where TS stripping happens before the Qwik fold.

**Where in pipeline (lib.rs):** After `parse_module()` and `SemanticBuilder`, but BEFORE `collector::collect()` and `traverse_mut()`:

```
parse_module -> SemanticBuilder -> [TS_STRIP_HERE] -> collector::collect -> traverse_mut
```

**Critical detail:** The Transformer consumes `Scoping`, so after TS stripping we need to rebuild `SemanticBuilder` for the stripped program. The pipeline becomes:

```rust
// In lib.rs, after parse_module:
if transform_options.transpile_ts && parse_result.source_type.is_typescript() {
    let ts_options = oxc::transformer::TransformOptions {
        typescript: oxc::transformer::TypeScriptOptions::default(),
        ..Default::default()
    };
    let transformer = oxc::transformer::Transformer::new(
        &allocator,
        std::path::Path::new(&input.path),
        &ts_options,
    );
    let _ts_result = transformer.build_with_scoping(scoping, &mut program);
    // Rebuild scoping for the stripped AST
    let semantic_ret = oxc::semantic::SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .build(&program);
    scoping = semantic_ret.semantic.into_scoping();
}
```

**Confidence:** HIGH -- verified that `oxc::transformer::Transformer::build_with_scoping` takes `Scoping` + `&mut Program`, which exactly matches what we have. SWC's approach is identical in concept (run TS strip before Qwik transform).

### BUG-02: Component Options Object Preservation

**What:** `component$(() => {...}, { tagName: "my-foo" })` should become `componentQrl(qrl(...), { tagName: "my-foo" })`, but OXC only passes the QRL as the first argument, dropping additional arguments.

**Root cause (transform.rs lines 1844-1873):** When building the final `componentQrl(...)` call in `exit_expression`, only 1 argument is pushed:
```rust
let mut args = ctx.ast.vec_with_capacity(1);
args.push(Argument::from(replacement));
```

SWC (transform.rs lines 3238-3258) processes ALL arguments: args at index 0 are replaced with the QRL call, but args at index > 0 are passed through with `arg.fold_with(self)`.

**Fix:** After pushing the QRL replacement as arg[0], iterate over the original call's arguments starting from index 1 and push them through:
```rust
let mut args = ctx.ast.vec_with_capacity(call.arguments.len());
args.push(Argument::from(replacement));
// Pass through additional arguments (e.g., { tagName: "my-foo" })
for i in 1..call.arguments.len() {
    let placeholder = Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
    let extra_arg = std::mem::replace(&mut call.arguments[i], placeholder);
    args.push(extra_arg);
}
```

**Important:** This fix applies to ALL named $-suffixed calls, not just `component$`. SWC passes through all extra args for all marker functions.

**Confidence:** HIGH -- directly compared SWC and OXC source code.

### BUG-03: Capture Variables Mismatch

**What:** Captured variable lists differ between SWC and OXC in two distinct categories:

1. **Variables captured that shouldn't be (or vice versa):** This is primarily caused by missing Phase 4 features. When OXC doesn't transform `_wrapProp(state, "count")` (it leaves `state.count`), the `state` variable appears as a capture instead of an import. Similarly, props destructuring differences cause `data` to be captured instead of `_rawProps`.

2. **Extra loop iteration variables captured:** In tests like `example_component_with_event_listeners_inside_loop`, OXC captures `i`, `item`, `key`, etc. that SWC doesn't. SWC has `iteration_var_stack` + `loop_depth` tracking that injects iteration variables as `q:p` parameters rather than captures. This is deferred to Phase 4 (the 23 event handler paramNames mismatches).

**Genuine BUG-03 capture issues (not Phase 4 features):**
- Capture ordering: OXC uses identifier encounter order during traverse. SWC uses a different traversal order (fold vs traverse). When the same variables are captured but in different order, the `_captures[N]` indices don't match.
- Module-level declaration reclassification edge cases.

**Analysis of capture diffs from actual snapshots:**
- Most capture name differences are actually caused by missing `_wrapProp` / `_fnSignal` transforms (Phase 4)
- Loop variable captures (`i`, `item`) are caused by missing `q:p` injection (Phase 4)
- Some genuine ordering differences exist due to SWC fold (top-down) vs OXC traverse (visitor pattern)

**Recommendation:** During Phase 3, focus ONLY on verifiable capture ordering bugs where the same set of variables is captured but in a different order. Defer capture content changes to Phase 4 when the transforms that produce those captures exist. Run a diff analysis after BUG-01 and BUG-02 fixes to see how many pure capture-order issues remain.

**Confidence:** MEDIUM -- many of these will resolve when Phase 4 features land. Need post-fix diff analysis.

### BUG-04: Missing Segments

**What:** Some test cases produce fewer segment files in OXC than SWC. Two distinct root causes identified:

1. **Multi-file inputs:** The `relative_paths` test uses `additional_inputs` (a second file). The SWC golden snapshot shows segments from both files. Verified that the OXC test harness does pass both files through `transform_modules()`, but the second file may not be generating segments correctly.

2. **Segment ordering / naming differences:** Some segments appear with different names (due to Phase 1 naming fixes not covering all cases), making them look "missing" when they're actually present under a different name.

3. **JSX event handler segments not created:** In some cases where JSX transpilation produces different event handler patterns, segments that SWC creates from JSX events are missing in OXC.

**Investigation approach:** After BUG-01 and BUG-02 fixes, re-run snapshot diffs and count which tests still have a different number of `=== ... === (ENTRY)` lines. For each, determine:
- Is the segment under a different name?
- Is the segment from a secondary input file?
- Is the segment from a JSX event handler that wasn't detected?

**Confidence:** MEDIUM -- root causes are known but specific instances need case-by-case investigation.

### BUG-05: qwik_router_inline Test Fixture

**What:** The `example_qwik_router_inline` test has a 2173-line diff because the OXC test uses a 16-line placeholder file (`tests/input/example_qwik_router_inline.tsx`) while SWC uses the actual qwik router bundle (`fixtures/index.qwik.mjs`, 1074 lines).

**Root cause:** The OXC test fixture is a simple `component$` example, NOT the real qwik-router compiled code. The SWC test uses `include_str!("fixtures/index.qwik.mjs")` to load the actual pre-compiled qwik-router source.

**Fix:** Copy `crates/swc-optimizer/core/src/fixtures/index.qwik.mjs` to `crates/qwik-optimizer-oxc/tests/input/example_qwik_router_inline.tsx` (or a new path referenced by the test). The test configuration already sets the correct options (`EntryStrategy::Smart`, `explicit_extensions: true`, filename `../node_modules/@qwik.dev/router/index.qwik.mjs`).

**Important:** This is a pre-compiled `.mjs` file -- it already uses `componentQrl`, `inlinedQrl`, etc. directly (NOT `component$`). The optimizer must still handle it correctly because it uses `implicit$FirstArg` patterns and other Qwik-internal constructs. Significant OXC behavior differences may emerge when the real fixture is used, potentially revealing new bugs in segment extraction, import handling, or the Smart entry strategy.

**Confidence:** HIGH -- verified by comparing SWC test.rs line 3259 with OXC test.rs line 1062.

### BUG-06: Source Comments Not Preserved

**What:** Comments like `// Double count watch` present in SWC output are missing in OXC output. Affects both main module output and extracted segment code.

**Root causes:**

1. **Main module output (emit.rs):** OXC codegen (`oxc::codegen::Codegen`) with default `CodegenOptions` should preserve normal comments when `with_source_text()` is called. The `emit_module` function in `emit.rs` DOES call `.with_source_text(source)`, so main module comments should be preserved IF the AST nodes retain their original spans. However, when AST nodes are replaced during transformation (e.g., `component$()` -> `componentQrl()`), comments attached to replaced nodes are lost because the new synthetic nodes have `SPAN` (zero span).

2. **Segment body code (transform.rs line 1796):** `Codegen::new()` is called WITHOUT `.with_source_text()`. OXC codegen needs the original source text to emit comments (it reads them from the source using span positions). Without source text, NO comments are emitted in segment bodies.

3. **Segment module re-parse (code_move.rs line 308):** `emit_segment_with_map` re-parses the raw segment code string and re-emits it. Even if comments were present in the raw string, the re-parse step does not include comment preservation setup (it uses `source_in_arena` as source text but the comments are in the raw code string from the serialization step, which already lost them).

**SWC approach:** SWC preserves comments by passing `leading_comments` and `trailing_comments` (`SingleThreadedCommentsMap`) through the entire pipeline. Comments are tracked by span position and forwarded to the segment module construction (`code_move.rs::NewModuleCtx`). SWC's `code_move::new_module` accepts comment maps and creates a `SingleThreadedComments` from them, which the SWC emitter then uses.

**Fix approach for OXC:**

For main module: The main module emit path already uses `with_source_text()`, so comments on untransformed code should be preserved. Verify this is working correctly. For transformed nodes (synthetic AST), comments are inherently lost since the original spans are gone.

For segment bodies: Pass the original source text to `Codegen::new().with_source_text(&self.source_code)` when serializing segment body code at transform.rs line 1796. This should allow OXC codegen to emit comments from the source that fall within the arrow function body span range. Note: this only works for comments whose spans fall within the serialized expression's span range.

**Confidence:** MEDIUM -- the main module path should be straightforward. Segment body comment preservation depends on OXC codegen's ability to emit comments for sub-expressions when given the full source text. This needs testing.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| TypeScript type stripping | Manual AST node removal (interfaces, type annotations, type aliases, type-only imports, enums, etc.) | `oxc::transformer::Transformer` with TypeScript-only config | TS stripping has dozens of edge cases: type-only imports, const enums, declare keywords, interface/type declarations, parameter type annotations, return type annotations, generic type parameters, type assertions, satisfies operator, etc. |
| Comment tracking system | Custom comment map implementation | OXC's built-in comment system via `Codegen.with_source_text()` and `CommentOptions` | OXC already tracks comments by span position during parsing. Using `with_source_text()` enables the codegen to emit them. |

## Common Pitfalls

### Pitfall 1: TS Stripping Destroys Semantic Scoping
**What goes wrong:** `oxc::transformer::Transformer::build_with_scoping` consumes the `Scoping`. After TS stripping, the AST has changed (interfaces removed, type annotations stripped) but the scoping data is stale/consumed.
**Why it happens:** Transformer modifies the AST in place and invalidates scope/symbol data.
**How to avoid:** Rebuild `SemanticBuilder` after TS stripping, before the Qwik transform pass.
**Warning signs:** Panic or incorrect scope analysis in the Qwik transform; incorrect capture analysis.

### Pitfall 2: Component Options Fix Must Handle ALL Named Calls
**What goes wrong:** Only fixing `component$` to pass extra args, missing other functions.
**Why it happens:** Thinking only `component$` has extra args.
**How to avoid:** SWC passes through ALL args at index > 0 for ALL marker functions (the `fold_call_expr` enumerate loop). The fix should be generic.
**Warning signs:** Other $-suffixed calls with extra args being broken.

### Pitfall 3: Capture Diffs Are Mostly Phase 4 Symptoms
**What goes wrong:** Spending time debugging capture differences that are actually caused by missing `_wrapProp`, `_fnSignal`, or props destructuring transforms.
**Why it happens:** Without the signal/prop transforms (Phase 4), OXC produces different code that references different variables, causing different captures.
**How to avoid:** After fixing BUG-01 and BUG-02, re-run diffs and categorize remaining capture issues into: (a) pure ordering bugs, (b) Phase 4 feature gaps. Only fix (a) in Phase 3.
**Warning signs:** Capture names like `state` appearing instead of `_rawProps`, or loop vars `i`/`item` appearing -- these are Phase 4 issues.

### Pitfall 4: qwik_router_inline Fixture May Reveal New Bugs
**What goes wrong:** Replacing the test fixture with the real qwik-router code reveals many new failures beyond what BUG-05 describes.
**Why it happens:** The pre-compiled qwik-router uses patterns the optimizer hasn't handled (implicit$FirstArg, complex nesting, Smart entry strategy).
**How to avoid:** Treat BUG-05 as "replace fixture + diagnose", not "replace fixture + all diffs must be zero". Some diffs will be Phase 4/5/6 work.
**Warning signs:** Massive new diffs after fixture replacement that don't correspond to any Phase 3 bug.

### Pitfall 5: Segment Body Codegen Without Source Text Loses Comments
**What goes wrong:** Using `Codegen::new()` without `with_source_text()` produces code without comments.
**Why it happens:** OXC codegen reads comments from the source text by span position. Without the source text, it can't find any comments.
**How to avoid:** Always pass source text via `.with_source_text()` when comment preservation matters.
**Warning signs:** Segment modules missing all comments while main module preserves some.

## Code Examples

### BUG-01: Adding transformer feature and TS stripping (lib.rs)
```rust
// After parse_module, before collector::collect:
if transform_options.transpile_ts && parse_result.source_type.is_typescript() {
    use std::path::Path;
    let ts_options = oxc::transformer::TransformOptions {
        typescript: oxc::transformer::TypeScriptOptions::default(),
        ..Default::default()
    };
    let transformer = oxc::transformer::Transformer::new(
        &allocator,
        Path::new(&input.path),
        &ts_options,
    );
    let _ts_return = transformer.build_with_scoping(
        parse_result.scoping,
        &mut program,
    );
    // Rebuild semantic scoping for the stripped AST
    let semantic_ret = oxc::semantic::SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .build(&program);
    scoping = semantic_ret.semantic.into_scoping();
}
```

### BUG-02: Preserving component options (transform.rs exit_expression)
```rust
// In the DollarCallKind::Named(name) match arm, after building replacement:
let mut args = ctx.ast.vec_with_capacity(call.arguments.len().max(1));
args.push(Argument::from(replacement));
// Pass through additional arguments (e.g., { tagName: "my-foo" })
for i in 1..call.arguments.len() {
    let placeholder = Argument::from(
        ctx.ast.expression_identifier(SPAN, "undefined"),
    );
    let extra_arg = std::mem::replace(&mut call.arguments[i], placeholder);
    args.push(extra_arg);
}
```

### BUG-06: Adding source text to segment body codegen (transform.rs)
```rust
// At line 1796, change:
//   let mut codegen = oxc::codegen::Codegen::new();
// To:
let mut codegen = oxc::codegen::Codegen::new()
    .with_source_text(&self.source_code);
codegen.print_expression(expr_val);
```

### BUG-05: Cargo.toml feature addition
```toml
oxc = { version = "0.113", features = [
    "codegen",
    "semantic",
    "serialize",
    "ast_visit",
    "transformer",  # NEW for BUG-01: TypeScript type stripping
] }
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| SWC `typescript::strip()` | `oxc::transformer::Transformer` with TS config | OXC 0.50+ | Same concept, different API |
| SWC comment maps (`SingleThreadedCommentsMap`) | OXC span-based comments via `with_source_text()` | OXC design | Different mechanism, same goal |
| String-based segment code construction | Same (OXC approach) | Current | Comments are lost in string serialization |

## Open Questions

1. **Comment preservation in segment bodies via `with_source_text()`**
   - What we know: `Codegen.with_source_text()` enables comment output based on span positions
   - What's unclear: When printing a sub-expression (arrow function body), does OXC codegen include comments from the source text whose spans fall within that expression's range?
   - Recommendation: Test empirically. If it doesn't work, the fallback is to manually extract comments from the source code by span range and inject them into the segment code string.

2. **Transformer feature binary size impact**
   - What we know: The Cargo.toml comment says `"transformer"` is excluded because it "pulls in babel-compat layers we don't need"
   - What's unclear: Actual binary size increase from enabling the feature
   - Recommendation: Enable it and measure. The alternative (manual TS stripping) is far more complex and error-prone. Binary size can be optimized later.

3. **Segment ordering (SWC fold vs OXC traverse)**
   - What we know: 3 tests have same segment names in different order (traversal difference)
   - What's unclear: Whether this matters for runtime behavior (segments are loaded by name/hash, not position)
   - Recommendation: If ordering doesn't affect runtime, accept OXC's ordering. If snapshots must match exactly, sort segments by name before output in lib.rs.

4. **How many BUG-03 capture issues remain after Phase 4?**
   - What we know: Most capture diffs are caused by missing Phase 4 transforms
   - What's unclear: How many genuine capture ordering bugs exist
   - Recommendation: Do a diff triage after BUG-01/BUG-02 fixes. Only fix provable capture bugs in Phase 3.

## Bug-to-Snapshot Impact Analysis

Based on actual snapshot diff analysis:

| Bug | Affected Snapshots | Complexity | Dependencies |
|-----|--------------------|------------|--------------|
| BUG-01 (TS stripping) | ~20+ (all transpile_ts=true tests) | MEDIUM (Cargo feature + pipeline change) | None |
| BUG-02 (component options) | ~10 (tagName tests) | LOW (5-line fix) | None |
| BUG-03 (captures) | ~50 nominal, but ~40 are Phase 4 symptoms | LOW-MEDIUM | Phase 4 for most |
| BUG-04 (missing segments) | ~30 nominal (overlaps with naming) | MEDIUM | BUG-01 may resolve some |
| BUG-05 (qwik_router_inline) | 1 (2173-line diff) | MEDIUM (fixture + diagnostics) | BUG-01, multiple features |
| BUG-06 (comments) | ~5 | LOW-MEDIUM | None |

## Sources

### Primary (HIGH confidence)
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/transform.rs` (direct analysis, lines 1844-1873 for BUG-02)
- SWC optimizer source code: `crates/swc-optimizer/core/src/transform.rs` (lines 3238-3258 for SWC arg handling)
- SWC optimizer source code: `crates/swc-optimizer/core/src/parse.rs` (line 244 for `typescript::strip()`)
- SWC optimizer source code: `crates/swc-optimizer/core/src/code_move.rs` (lines 9, 30-38 for comment maps)
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/code_move.rs` (segment code generation)
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/emit.rs` (main module emission)
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/lib.rs` (transform pipeline)
- Snapshot diffs: `git diff -- crates/qwik-optimizer-oxc/tests/snapshots/` (full diff analysis)

### Secondary (MEDIUM confidence)
- [OXC CodegenOptions docs](https://docs.rs/oxc_codegen/latest/oxc_codegen/struct.CodegenOptions.html) -- comment preservation options
- [OXC CommentOptions docs](https://docs.rs/oxc_codegen/latest/oxc_codegen/struct.CommentOptions.html) -- normal, jsdoc, annotation, legal
- [OXC Transformer docs](https://docs.rs/oxc_transformer/0.113.0/oxc_transformer/struct.Transformer.html) -- build_with_scoping API
- [OXC TypeScriptOptions docs](https://docs.rs/oxc_transformer/0.113.0/oxc_transformer/struct.TypeScriptOptions.html) -- TS stripping config
- [OXC TransformOptions docs](https://docs.rs/oxc/0.113.0/oxc/transformer/struct.TransformOptions.html) -- top-level transform config
- [OXC TypeScript transformer docs](https://oxc.rs/docs/guide/usage/transformer/typescript) -- TS transform usage

### Tertiary (LOW confidence)
- Binary size impact of `"transformer"` feature -- not measured, recommendation to test

## Metadata

**Confidence breakdown:**
- BUG-01 (TS stripping): HIGH -- approach verified against SWC reference and OXC API docs
- BUG-02 (component options): HIGH -- root cause identified in exact source lines
- BUG-03 (captures): MEDIUM -- many diffs are Phase 4 symptoms, true Phase 3 scope unclear
- BUG-04 (missing segments): MEDIUM -- multiple root causes, needs case-by-case investigation
- BUG-05 (qwik_router_inline): HIGH -- test fixture mismatch confirmed
- BUG-06 (comments): MEDIUM -- approach identified but segment body comment preservation needs testing

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (stable domain, OXC 0.113 pinned)
