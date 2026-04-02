# Architecture

## Pipeline Overview

The Qwik optimizer transforms a single input file into multiple output modules:

```
Input (JS/TS/JSX/TSX)
    │
    ▼
┌─────────────────────────────┐
│  Phase 0: PARSE             │
│  oxc_parser → Program<'a>   │
│  SemanticBuilder → Scoping  │
└──────────────┬──────────────┘
               │
               ▼
┌─────────────────────────────┐
│  Phase 1: ANALYZE           │
│  Single read-only Traverse  │
│  Detect $-boundaries        │
│  Collect imports/exports    │
│  Capture analysis           │
│  → TransformPlan            │
└──────────────┬──────────────┘
               │
               ▼
┌─────────────────────────────┐
│  Phase 2: EMIT              │
│  Mutate main module AST     │
│  Build segment Programs     │
│  Codegen all programs       │
│  → TransformOutput          │
└─────────────────────────────┘
```

## Two-Phase Constraint

OXC's semantic analysis (`oxc_semantic`) becomes stale after AST mutation. All scope/symbol queries must happen in Phase 1 (analyze) before any mutation in Phase 2 (emit). This is fundamentally different from SWC where `Fold` interleaves analysis and mutation.

## Multi-Output

The optimizer's defining feature: one input produces N output modules (1 main + M segments). Each segment is a lazy-loadable chunk extracted from a `$()` boundary.

- Main module: original file with `$()` calls replaced by `qrl()` wrappers
- Segment modules: extracted functions with their own imports and exports

Segment `Program` ASTs are built from scratch using `AstBuilder` after traversal, not cloned from the original.

## Public API

```rust
pub fn transform_modules(options: TransformModulesOptions) -> Result<TransformOutput>
```

- **Input:** `TransformModulesOptions` with source code, filename, config flags
- **Output:** `TransformOutput` with `Vec<TransformModule>` (main + segments) and `Vec<Diagnostic>`

## Key Abstractions

| Type | Purpose |
|------|---------|
| `TransformModulesOptions` | All configuration for a transform call |
| `TransformOutput` | Complete result with modules and diagnostics |
| `TransformModule` | Single output module (code, source map, metadata) |
| `SegmentAnalysis` | Metadata about an extracted segment (name, hash, captures, etc.) |
| `EntryStrategy` | How segments are grouped (Segment, Inline, Smart, Hook, Component, Hoist) |
| `EmitMode` | Output mode (Lib, Dev, Prod) |
