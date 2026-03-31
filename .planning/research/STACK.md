# Technology Stack

**Project:** Qwik Optimizer -- OXC Port (v2.0 Optimizer Implementation Stack)
**Researched:** 2026-02-10
**Confidence:** HIGH (verified via docs.rs, GitHub source, crates.io, OXC official docs)

## Executive Context

This document defines the Rust crate stack for building the OXC-based Qwik optimizer. The previous v1.0 STACK.md covered parsing and serialization for spec generation. This document covers the **full optimizer pipeline**: AST traversal, semantic analysis, AST mutation/construction, code generation, source map generation, and supporting utilities.

The optimizer must: parse JS/TS/JSX/TSX input, analyze scope and variable captures, traverse and mutate the AST (extracting `$()` segments, converting `component$` to `componentQrl`, transforming JSX), generate JavaScript output from modified ASTs, and produce source maps for each output module.

## Recommended Stack

### Core Framework: OXC Umbrella Crate

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| `oxc` (umbrella) | `0.113` | Single dependency for all OXC sub-crates | All OXC sub-crates are versioned in lockstep. The umbrella crate with feature flags avoids version-sync nightmares. Already used in `oxc-ast-util` at this version. |
| Rust | edition 2024 | Language edition | OXC 0.113 itself uses edition 2024 and requires MSRV 1.91.0. The existing `oxc-ast-util` crate already uses edition 2024. |

### OXC Feature Flags Required

```toml
[dependencies]
oxc = { version = "0.113", features = [
    "semantic",     # Scope analysis, symbol table, reference resolution
    "codegen",      # AST-to-JavaScript code generation + source maps
    "serialize",    # serde::Serialize on AST types (for SegmentAnalysis JSON output)
] }
```

**Why these features:**

| Feature | Enables | Why Needed |
|---------|---------|------------|
| `semantic` | `oxc_semantic` (SemanticBuilder, Scoping, symbol table, scope tree, reference tracking) | **Capture analysis.** The optimizer must determine which variables a `$()` closure captures from its enclosing scope. This requires scope hierarchy traversal, symbol resolution, and reference tracking -- exactly what `oxc_semantic` provides via `Scoping::get_resolved_references()`, `find_binding()`, `scope_ancestors()`. |
| `codegen` | `oxc_codegen` + `oxc_codegen/sourcemap` (Codegen, CodegenOptions, CodegenReturn with SourceMap) | **Code generation.** Every extracted segment and the root module must be emitted as JavaScript source. The `codegen` feature also enables the `sourcemap` sub-feature, providing `oxc_sourcemap::SourceMap` in `CodegenReturn.map`. |
| `serialize` | `serde::Serialize` on all AST types, span types, syntax types | **JSON output.** The `TransformOutput` API includes `SegmentAnalysis` as JSON. Serialization is needed to produce this metadata. Also useful for debugging and test snapshots. |

**Why NOT these features:**

| Feature | Why Skip |
|---------|----------|
| `minifier` | The optimizer does not minify. Minification is handled by the downstream bundler (Vite/Rollup). |
| `mangler` | Same as minifier -- not the optimizer's responsibility. |
| `full` | Pulls in transformer, minifier, mangler, isolated_declarations. All unnecessary, adds compile time. |
| `ast_visit` | Provides `Visit`/`VisitMut` traits for simple read-only or read-write traversal. These lack parent context access. The optimizer needs parent context for capture analysis (knowing which scope a reference belongs to). Use `oxc_traverse` (included automatically when using `semantic`) instead. **However**, `ast_visit` may be useful for simple analysis passes that don't need parent context. Add it only if a concrete need arises. |
| `cfg` | Control flow graph analysis. Not needed for Qwik's transformations. |
| `isolated_declarations` | TypeScript declaration file generation. Not the optimizer's job. |

### OXC Sub-crates Accessed via Umbrella (no separate Cargo.toml entries)

These are all re-exported through the `oxc` umbrella crate. Understanding them individually is critical for the optimizer's architecture.

#### Foundation (always available)

| Sub-crate | Accessed As | Purpose for Optimizer |
|-----------|-------------|----------------------|
| `oxc_allocator` | `oxc::allocator::Allocator` | Arena bump allocator. All AST nodes live in an `Allocator` arena with lifetime `'a`. Each file transformation needs its own allocator. |
| `oxc_parser` | `oxc::parser::{Parser, ParseOptions, ParserReturn}` | Parse input JS/TS/JSX/TSX to `Program<'a>`. Returns `ParserReturn` with `program`, `module_record`, `errors`. |
| `oxc_ast` | `oxc::ast::ast::*`, `oxc::ast::AstBuilder` | AST type definitions and **AstBuilder** for constructing new AST nodes. AstBuilder is the primary tool for creating replacement nodes during transformation. |
| `oxc_span` | `oxc::span::{SourceType, Span, Atom}` | Source type detection, span tracking, interned string atoms. |
| `oxc_syntax` | `oxc::syntax::*` | Operator types, number bases, language-level utilities. |
| `oxc_diagnostics` | `oxc::diagnostics::OxcDiagnostic` | Error/warning types for parse and semantic errors. |

#### Semantic Analysis (via `semantic` feature)

| Sub-crate | Accessed As | Purpose for Optimizer |
|-----------|-------------|----------------------|
| `oxc_semantic` | `oxc::semantic::{SemanticBuilder, Semantic, Scoping}` | **Critical for capture analysis.** Builds symbol table and scope tree. Provides: `Scoping::get_resolved_references(symbol_id)` to find all uses of a variable, `find_binding(scope_id, name)` to resolve identifiers up the scope chain, `scope_ancestors(scope_id)` to walk up scope hierarchy, `symbol_declaration(symbol_id)` to find where a symbol is declared. |
| `oxc_traverse` | `oxc::traverse::{traverse_mut, Traverse, TraverseCtx}` | **Primary transformation API.** Implements visitor pattern with parent context access. `TraverseCtx` provides: `ctx.ast` (AstBuilder for creating new nodes), `ctx.parent()` (ancestor access), scope/symbol info. Transformer visitors implement `Traverse<'a>` trait with `enter_*`/`exit_*` methods receiving `&mut` AST node references. |

#### Code Generation (via `codegen` feature)

| Sub-crate | Accessed As | Purpose for Optimizer |
|-----------|-------------|----------------------|
| `oxc_codegen` | `oxc::codegen::{Codegen, CodegenOptions, CodegenReturn}` | Emit JavaScript from transformed AST. `Codegen::new().with_options(opts).build(&program)` returns `CodegenReturn { code: String, map: Option<SourceMap>, legal_comments }`. |
| `oxc_sourcemap` | `oxc::sourcemap::{SourceMap, ConcatSourceMapBuilder, SourceMapBuilder}` | Source map creation and manipulation. `SourceMap::to_json_string()` for JSON output, `to_data_url()` for inline embedding. `ConcatSourceMapBuilder` for combining source maps from multiple output modules. |

### Supporting Rust Libraries

| Library | Version | Purpose | Why This One |
|---------|---------|---------|--------------|
| `serde` | `1.0` | Serialization framework | Required by OXC's `serialize` feature. Also needed for `SegmentAnalysis` JSON output and `TransformModulesOptions` deserialization. |
| `serde_json` | `1.0` | JSON serialization/deserialization | Serialize `SegmentAnalysis` metadata, deserialize transform options from the TypeScript layer. |
| `anyhow` | `1.0` | Error handling | Existing SWC optimizer uses `anyhow`. Provides ergonomic `Result<T>` with context chaining. Better than `Box<dyn Error>` for application-level code. |
| `rayon` | `1.10` | Data parallelism | Existing SWC optimizer uses `rayon` for parallel file transformation. The optimizer processes multiple input modules concurrently. Each module gets its own `Allocator`, so arena lifetimes don't conflict across threads. |
| `base64` | `0.22` | Base64 encoding | Encode source maps for inline embedding (`//# sourceMappingURL=data:application/json;base64,...`). The existing SWC optimizer uses this. Note: `oxc_sourcemap::SourceMap::to_data_url()` may handle this internally, but explicit `base64` is needed if the optimizer constructs source map URLs manually. |
| `pathdiff` | `0.2` | Relative path calculation | Calculate relative paths between source files and output modules. Used in source map `sources` field and module path resolution. Existing SWC optimizer dependency. |
| `path-slash` | `0.2` | Cross-platform path normalization | Convert Windows `\` paths to POSIX `/` paths. Required for consistent module IDs across platforms. Existing SWC optimizer dependency. |
| `rustc-hash` | `2.1` | Fast hash maps (FxHashMap) | Performance-critical hash maps for symbol tables, capture sets, module registries. OXC itself uses FxHashMap internally. Deterministic, faster than std HashMap for small keys (identifiers, symbol IDs). Not cryptographically secure, but the optimizer doesn't need security. |
| `indexmap` | `2.7` | Insertion-ordered hash map | Preserve deterministic output ordering for segments and exports. When iterating over extracted segments, the order must be stable for reproducible builds. OXC uses `indexmap` as a workspace dependency. |

### Testing Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `insta` | `1.29+` | Snapshot testing | Test that optimizer output matches expected snapshots. Use `assert_snapshot!` for generated code, `assert_json_snapshot!` for `SegmentAnalysis` JSON. Existing project dependency. |
| `serde_json` | (same as above) | Test fixture parsing | Parse test input configurations from JSON. |

### What NOT to Add

| Crate | Why Avoid | What to Use Instead |
|-------|-----------|-------------------|
| Individual `oxc_parser`, `oxc_ast`, etc. | Version sync nightmare across 10+ crates that must all be `0.113.x`. | `oxc` umbrella crate with feature flags. |
| `swc_common`, `swc_ecmascript`, `swc_atoms` | The entire point is replacing SWC. These are the crates being removed. | OXC equivalents. |
| `lazy_static` | Outdated. `std::sync::LazyLock` is stable since Rust 1.80 (we require 1.91+ for OXC). | `std::sync::LazyLock` or `std::sync::OnceLock`. |
| `derivative` | Proc macro for custom derives. OXC AST types already implement necessary traits. Use `#[derive(...)]` directly. | Standard derives. |
| `simple-error` | Existing SWC optimizer uses it, but `anyhow` covers all error handling needs. | `anyhow`. |
| `serde_bytes` | Existing SWC optimizer uses it for binary source map data. OXC's `SourceMap` handles serialization internally via `to_json_string()`. | `oxc_sourcemap::SourceMap::to_json_string()`. |
| `relative-path` | Overlaps with `pathdiff`. One path library is sufficient. | `pathdiff` + `path-slash`. |
| `oxc_transformer` / `transformer` feature | OXC's transformer does ES downleveling/TS stripping -- not Qwik transforms. Using it would fight against the optimizer's custom transformation logic. | Custom `Traverse` implementation. |
| `tree-sitter`, `syn`, `quote` | Wrong abstraction level. `tree-sitter` is for editors, `syn`/`quote` are for Rust proc macros. | OXC's own AST and AstBuilder. |

## Key Integration Points Between Crates

### Parse -> Semantic -> Traverse -> Codegen Pipeline

```
Input Source (JS/TS/JSX/TSX)
    |
    v
[oxc_parser] Parser::new(&allocator, source, source_type).parse()
    |   Returns: ParserReturn { program, module_record, errors }
    v
[oxc_semantic] SemanticBuilder::new().build(&program)
    |   Returns: Semantic { scoping, ast_nodes }
    |   Provides: scope tree, symbol table, reference resolution
    v
[oxc_traverse] traverse_mut(&mut program, &mut visitor, &mut ctx)
    |   Visitor implements Traverse<'a> trait
    |   ctx.ast provides AstBuilder for node construction
    |   ctx.parent() provides ancestor access
    |   Visitor mutates AST: extracts segments, wraps QRLs, transforms JSX
    v
[oxc_codegen] Codegen::new().with_options(opts).build(&program)
    |   Returns: CodegenReturn { code, map, legal_comments }
    |   One codegen pass per output module (root + each segment)
    v
Output: TransformOutput { modules: Vec<TransformModule>, diagnostics }
```

### How Semantic Analysis Feeds Capture Analysis

```rust
// 1. Parse
let ret = Parser::new(&allocator, source, source_type).parse();

// 2. Build semantic info
let semantic = SemanticBuilder::new().build(&ret.program);
let scoping = semantic.scoping;

// 3. For each $() call found during traversal:
//    - Identify the closure's scope_id
//    - Walk all IdentifierReferences in the closure body
//    - For each reference, check if its symbol is declared OUTSIDE the closure scope
//    - If outside: it's a capture. Add to capture set.
//
// Key Scoping API calls:
//    scoping.find_binding(scope_id, name)     -> find where a name resolves
//    scoping.get_resolved_references(sym_id)  -> find all uses of a symbol
//    scoping.scope_ancestors(scope_id)        -> walk up scope chain
//    scoping.symbol_declaration(sym_id)       -> get declaring AST node
```

### How AstBuilder Creates Replacement Nodes

```rust
// Inside a Traverse visitor's enter_* method:
fn enter_expression(&mut self, expr: &mut Expression<'a>, ctx: &mut TraverseCtx<'a>) {
    // Example: Replace component$(fn) with componentQrl(qrl(...))
    //
    // ctx.ast is the AstBuilder instance
    // Use it to construct new nodes:

    // Create: qrl(() => import("./segment.js"), "symbolName")
    let import_fn = ctx.ast.expression_arrow_function(/* ... */);
    let symbol_name = ctx.ast.expression_string_literal(SPAN, "symbolName", None);
    let qrl_call = ctx.ast.expression_call(
        SPAN,
        ctx.ast.expression_identifier_reference(SPAN, "qrl"),
        /* type_parameters */ None,
        ctx.ast.vec_from_iter([
            Argument::from(import_fn),
            Argument::from(symbol_name),
        ]),
        /* optional */ false,
    );

    // Replace the original expression
    *expr = qrl_call;
}
```

### How Codegen Produces Source Maps

```rust
use oxc::codegen::{Codegen, CodegenOptions, CodegenReturn};
use std::path::PathBuf;

let options = CodegenOptions {
    source_map_path: Some(PathBuf::from("input.tsx")),
    // Sets the "source" field in the returned sourcemap
    ..CodegenOptions::default()
};

let CodegenReturn { code, map, .. } = Codegen::new()
    .with_options(options)
    .with_source_text(original_source)
    .build(&program);

// map is Option<SourceMap>
if let Some(source_map) = map {
    // JSON string for external source map file
    let json = source_map.to_json_string();

    // Or inline as data URL
    let data_url = source_map.to_data_url();
    // Produces: data:application/json;base64,...
}
```

### Combining Source Maps from Multiple Output Modules

```rust
use oxc::sourcemap::ConcatSourceMapBuilder;

// When the optimizer produces multiple output modules from one input,
// each module's codegen produces its own SourceMap.
// ConcatSourceMapBuilder can combine them if needed:

let builder = ConcatSourceMapBuilder::from_sourcemaps(&[
    (&root_module_map, 0),           // Root module starts at line 0
    (&segment_1_map, root_lines),    // Segment 1 offset by root module line count
]);
let combined = builder.into_sourcemap();
```

## OXC Allocator Lifetime Management

**The single most important constraint** for the optimizer's architecture:

```rust
// CRITICAL: All AST nodes borrow from the Allocator.
// The Allocator MUST outlive all AST references.

fn transform_module(source: &str, filename: &str) -> TransformOutput {
    // Each module transformation gets its own allocator
    let allocator = Allocator::default();

    // Parse (program borrows allocator)
    let ret = Parser::new(&allocator, source, SourceType::from_path(filename).unwrap())
        .parse();
    let mut program = ret.program;

    // Semantic analysis (borrows program, which borrows allocator)
    let semantic = SemanticBuilder::new().build(&program);

    // Traverse and mutate (modifies program in-place)
    // AstBuilder accessed via ctx.ast also allocates in the same arena
    traverse_mut(&mut program, &mut my_visitor, /* ... */);

    // Codegen (reads program, produces owned String)
    let result = Codegen::new()
        .with_options(codegen_opts)
        .build(&program);

    // result.code is an owned String -- it does NOT borrow the allocator
    // Safe to return after allocator is dropped
    TransformOutput {
        code: result.code,
        map: result.map,  // SourceMap is also owned
    }
}
// allocator dropped here -- all AST memory freed
```

**For parallel processing with rayon:**

```rust
use rayon::prelude::*;

let outputs: Vec<TransformOutput> = input_modules
    .par_iter()
    .map(|module| {
        // Each parallel task gets its own allocator
        // No lifetime conflicts between threads
        transform_module(&module.source, &module.filename)
    })
    .collect();
```

## Version Compatibility Matrix

| Package | Version | Compatible With | Notes |
|---------|---------|-----------------|-------|
| `oxc` | 0.113.x | Rust edition 2024, MSRV 1.91.0 | OXC releases weekly. Pin to `0.113` (allow patch). |
| `oxc` | 0.113.x | `serde` 1.x, `serde_json` 1.x | OXC's `serialize` feature generates `serde::Serialize` impls. |
| `oxc` | 0.113.x | `rayon` 1.x | No conflict. Each thread gets own `Allocator`. |
| `rayon` | 1.10+ | Rust 1.63+ | Well within our MSRV 1.91. |
| `rustc-hash` | 2.1 | Rust 1.36+ | FxHashMap/FxHashSet drop-in replacements for std HashMap/HashSet. |
| `indexmap` | 2.7+ | Rust 1.63+ | Used by OXC internally. Compatible versions. |
| `anyhow` | 1.0.100+ | Rust 1.39+ | Widely compatible. |
| `base64` | 0.22 | Rust 1.48+ | Standard base64 crate. |

## Installation

```toml
# oxc-optimizer/Cargo.toml

[package]
name = "oxc-optimizer"
version = "0.1.0"
edition = "2024"

[dependencies]
# Core: OXC umbrella crate with optimizer-needed features
oxc = { version = "0.113", features = ["semantic", "codegen", "serialize"] }

# Serialization (for TransformOutput JSON, SegmentAnalysis metadata)
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Error handling
anyhow = "1.0"

# Parallelism (multi-file transformation)
rayon = "1.10"

# Source map encoding
base64 = "0.22"

# Path utilities
pathdiff = "0.2"
path-slash = "0.2"

# Performance data structures
rustc-hash = "2.1"
indexmap = "2.7"

[dev-dependencies]
insta = { version = "1.29", features = ["json"] }
```

## Alternatives Considered

| Category | Recommended | Alternative | Why Not Alternative |
|----------|-------------|-------------|-------------------|
| AST Framework | `oxc` umbrella | Individual `oxc_*` crates | 10+ crates needing identical version pins. Umbrella solves this. |
| AST Framework | `oxc` | `swc_ecmascript` | SWC is what we're replacing. OXC is 3x faster parser, actively maintained, better DX. |
| Traversal | `oxc_traverse` (Traverse trait) | `oxc_ast_visit` (Visit/VisitMut traits) | Visit/VisitMut lack parent context. Capture analysis requires knowing which scope a reference belongs to. Traverse provides `ctx.parent()` and scope context. |
| Traversal | `oxc_traverse` | `oxc_transformer` (built-in transforms) | OXC transformer does ES downleveling/TS stripping. Qwik needs custom transforms (segment extraction, QRL wrapping). Using built-in transformer would fight against our logic. |
| Code generation | `oxc_codegen` | Manual string building | Codegen handles operator precedence, parenthesization, whitespace, comment preservation, and source map generation. Manual string building would be error-prone and miss source maps. |
| Source maps | `oxc_sourcemap` (included with `codegen`) | `sourcemap` (getsentry) | OXC's fork is optimized for OXC's pipeline, encodes in parallel, and integrates directly with `Codegen`. No format conversion needed. |
| Hash maps | `rustc-hash` (FxHashMap) | `std::collections::HashMap` | FxHashMap is 2-5x faster for string/integer keys. OXC uses it internally. The optimizer does millions of symbol lookups. |
| Hash maps | `rustc-hash` | `ahash` | Both are fast non-cryptographic hashes. `rustc-hash` is deterministic across platforms (important for reproducible builds). `ahash` randomizes per process. |
| Ordered maps | `indexmap` | `BTreeMap` | IndexMap preserves insertion order with O(1) lookup. BTreeMap sorts by key (wrong semantics -- we want insertion order for deterministic segment ordering). |
| Error handling | `anyhow` | `thiserror` | `thiserror` is for library error types. `anyhow` is for application-level error handling. The optimizer is an application (called from TypeScript layer), not a library consumed by other Rust crates. If we later publish optimizer internals as a library, add `thiserror` for the public API types. |
| Parallelism | `rayon` | `tokio` | `rayon` is for CPU-bound data parallelism (transforming files). `tokio` is for async I/O. The optimizer is CPU-bound -- no network or file I/O during transformation. |

## Crate-to-Optimizer-Capability Mapping

| Optimizer Capability | Primary Crate | Key API |
|---------------------|---------------|---------|
| Parse input files | `oxc_parser` | `Parser::new().parse()` |
| Detect module imports/exports | `oxc_parser` | `ParserReturn.module_record` |
| Build scope tree | `oxc_semantic` | `SemanticBuilder::new().build()` |
| Resolve variable references | `oxc_semantic` | `Scoping::find_binding()`, `get_resolved_references()` |
| Analyze captures in `$()` closures | `oxc_semantic` | `Scoping::scope_ancestors()`, `symbol_declaration()` |
| Traverse AST with parent context | `oxc_traverse` | `traverse_mut()`, `Traverse` trait |
| Create new AST nodes | `oxc_ast` | `AstBuilder` (accessed via `ctx.ast` in Traverse) |
| Replace/remove AST nodes | `oxc_traverse` | `enter_*`/`exit_*` with `&mut` node references |
| Generate JavaScript output | `oxc_codegen` | `Codegen::new().build(&program)` |
| Generate source maps | `oxc_codegen` | `CodegenOptions { source_map_path }` -> `CodegenReturn.map` |
| Combine source maps | `oxc_sourcemap` | `ConcatSourceMapBuilder` |
| Encode source maps to base64 | `oxc_sourcemap` + `base64` | `SourceMap::to_data_url()` or manual `base64::encode` |
| Serialize segment metadata | `serde` + `serde_json` | `#[derive(Serialize)]` on `SegmentAnalysis` |
| Parallel file processing | `rayon` | `par_iter().map(transform_module)` |
| Fast symbol/scope lookups | `rustc-hash` | `FxHashMap<Atom, SymbolId>` |
| Deterministic segment ordering | `indexmap` | `IndexMap<SegmentId, SegmentAnalysis>` |
| Cross-platform path handling | `pathdiff` + `path-slash` | `diff_paths()`, `PathBufExt::to_slash()` |
| Error propagation | `anyhow` | `anyhow::Result<T>`, `.context("msg")` |

## Migration from SWC Crates

| SWC Crate | OXC Replacement | Migration Notes |
|-----------|-----------------|-----------------|
| `swc_ecmascript` (parser) | `oxc_parser` | Parser API is similar. `Parser::new(&allocator, source, source_type).parse()`. Key difference: arena allocator lifetime. |
| `swc_ecmascript` (codegen) | `oxc_codegen` | `Codegen::new().build(&program)` instead of SWC's `Emitter`. Source maps via `CodegenOptions::source_map_path`. |
| `swc_ecmascript` (visit) | `oxc_traverse` | SWC uses `Visit`/`VisitMut`/`Fold`. OXC uses `Traverse` trait with `enter_*`/`exit_*`. The `Traverse` trait provides parent context that SWC lacks. |
| `swc_ecmascript` (utils) | `oxc_ast::AstBuilder` | SWC has misc helpers. OXC's AstBuilder provides hundreds of typed constructors for every AST node. |
| `swc_common` (sourcemap) | `oxc_sourcemap` | OXC's fork of `rust-sourcemap`, optimized for parallel encoding. |
| `swc_common` (Span, SourceMap) | `oxc_span` | `Span { start: u32, end: u32 }` instead of SWC's `Span { lo: BytePos, hi: BytePos }`. |
| `swc_atoms` (Atom) | `oxc_span::Atom` | Arena-inlined strings instead of globally interned atoms. No global lock contention in parallel processing. |
| `lazy_static` | `std::sync::LazyLock` | Standard library since Rust 1.80. No external crate needed. |
| `derivative` | `#[derive(...)]` | OXC AST types implement standard traits. Custom derives unnecessary. |
| `simple-error` | `anyhow` | Single error handling crate is sufficient. |
| `serde_bytes` | `oxc_sourcemap::SourceMap::to_json_string()` | OXC handles source map serialization internally. |

## Sources

- [oxc crate on crates.io](https://crates.io/crates/oxc) -- version 0.113.0, umbrella crate (HIGH confidence, verified)
- [oxc feature flags on lib.rs](https://lib.rs/crates/oxc/features) -- all feature flags documented (HIGH confidence)
- [OXC umbrella Cargo.toml](https://github.com/oxc-project/oxc/blob/main/crates/oxc/Cargo.toml) -- version 0.113.0, feature flag definitions (HIGH confidence)
- [OXC workspace Cargo.toml](https://github.com/oxc-project/oxc/blob/main/Cargo.toml) -- edition 2024, MSRV 1.91.0 (HIGH confidence)
- [OXC GitHub releases](https://github.com/oxc-project/oxc/releases) -- v0.113.0 released 2026-02-10 (HIGH confidence)
- [oxc_codegen Codegen docs.rs](https://docs.rs/oxc/latest/oxc/codegen/struct.Codegen.html) -- Codegen API (HIGH confidence)
- [oxc_codegen CodegenOptions docs.rs](https://docs.rs/oxc/latest/oxc/codegen/struct.CodegenOptions.html) -- source_map_path, minify, etc. (HIGH confidence)
- [oxc_codegen CodegenReturn docs.rs](https://docs.rs/oxc_codegen/latest/oxc_codegen/struct.CodegenReturn.html) -- code, map, legal_comments fields (HIGH confidence)
- [oxc_codegen example on GitHub](https://github.com/oxc-project/oxc/blob/main/crates/oxc_codegen/examples/codegen.rs) -- working example with source maps (HIGH confidence)
- [oxc_traverse docs.rs](https://docs.rs/oxc_traverse/latest/oxc_traverse/) -- Traverse trait, TraverseCtx, Ancestor (HIGH confidence)
- [oxc_traverse README](https://github.com/oxc-project/oxc/tree/main/crates/oxc_traverse) -- parent context, memory safety (HIGH confidence)
- [oxc_semantic docs.rs](https://docs.rs/oxc_semantic/latest/oxc_semantic/) -- SemanticBuilder, Scoping (HIGH confidence)
- [oxc_semantic Scoping docs.rs](https://docs.rs/oxc_semantic/latest/oxc_semantic/struct.Scoping.html) -- full method list (HIGH confidence)
- [oxc_ast AstBuilder docs.rs](https://docs.rs/oxc_ast/latest/oxc_ast/struct.AstBuilder.html) -- node construction API (HIGH confidence)
- [oxc_sourcemap docs.rs](https://docs.rs/oxc_sourcemap/latest/oxc_sourcemap/) -- SourceMap, ConcatSourceMapBuilder (HIGH confidence)
- [oxc_sourcemap SourceMap docs.rs](https://docs.rs/oxc_sourcemap/latest/oxc_sourcemap/struct.SourceMap.html) -- to_json_string, to_data_url, from_json (HIGH confidence)
- [OXC transformer architecture](https://github.com/oxc-project/oxc/blob/main/crates/oxc_transformer/src/lib.rs) -- how Traverse integrates with semantic (HIGH confidence)
- [OXC semantic analysis guide](https://oxc.rs/docs/learn/parser_in_rust/semantic_analysis) -- scope tree design (HIGH confidence)
- [OXC architecture overview](https://deepwiki.com/oxc-project/oxc) -- pipeline: parser -> semantic -> traverse -> codegen (MEDIUM confidence)
- [rustc-hash on GitHub](https://github.com/rust-lang/rustc-hash) -- FxHashMap, deterministic hashing (HIGH confidence)
- [indexmap docs.rs](https://docs.rs/crate/indexmap/latest) -- version 2.13.0 (HIGH confidence)
- [rayon docs.rs](https://docs.rs/rayon/latest/rayon/) -- version 1.11.0 (HIGH confidence)
- [pathdiff docs.rs](https://docs.rs/pathdiff/latest/pathdiff/) -- version 0.2.3 (HIGH confidence)
- [path-slash docs.rs](https://docs.rs/path-slash) -- cross-platform path conversion (HIGH confidence)
- [anyhow docs.rs](https://docs.rs/anyhow) -- version 1.0.100+ (HIGH confidence)

---
*Stack research for: Qwik optimizer OXC port -- implementation stack (v2.0)*
*Researched: 2026-02-10*
