# Technology Stack

## New Optimizer (qwik-optimizer-oxc)

- **Rust** (edition 2024, MSRV 1.91.0)
- **OXC umbrella crate** v0.113 with features: `semantic`, `codegen`, `serialize`
  - `oxc_parser` — Parse JS/TS/JSX/TSX to `Program<'a>`
  - `oxc_semantic` — Scope tree, symbol table, reference resolution (capture analysis)
  - `oxc_traverse` — `Traverse` trait with parent context for AST mutation
  - `oxc_ast` — `AstBuilder` for constructing new AST nodes
  - `oxc_codegen` — Generate JavaScript + source maps from AST
  - `oxc_sourcemap` — Source map manipulation
- **serde / serde_json** — JSON serialization for `SegmentAnalysis` metadata
- **rayon** — Parallel file transformation
- **anyhow** — Error handling
- **insta** — Snapshot testing (dev dependency)

## Old Optimizer (swc-optimizer, reference only)

- **SWC** (`swc_ecmascript`, `swc_common`, `swc_atoms`) — AST parsing/transformation/codegen
- **insta** — Snapshot testing

## Key Differences: SWC vs OXC

| Concern | SWC | OXC |
|---------|-----|-----|
| Traversal | `Fold` (ownership transfer) | `Traverse` (mutable reference + `TraverseCtx`) |
| Scope resolution | `SyntaxContext` inline on identifiers | `SemanticBuilder` pre-pass → `Scoping` API |
| Node creation | `Box::new(...)` heap alloc | `AstBuilder` arena alloc (lifetime `'a`) |
| Node replacement | Return new node from fold | `*node = new_node` assignment |
| Multi-output | Build separate `Module` during fold | Build separate `Program` after traversal |
