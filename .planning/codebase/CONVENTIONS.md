# Coding Conventions

## Rust

- **Edition:** 2024
- **MSRV:** 1.91.0 (required by OXC 0.113)
- **Error handling:** `anyhow::Result<T>` for application-level errors
- **Hash maps:** `rustc-hash::FxHashMap` for performance-critical lookups
- **Ordered maps:** `indexmap::IndexMap` for deterministic segment ordering

## Function Categories

Functions fall into two categories based on lifetime requirements:

1. **Pure analysis functions** — No AST creation, no `'a` lifetime needed. These analyze data and return plain Rust types (strings, vecs, booleans).

2. **AST construction functions** — Create/modify AST nodes, MUST take `&AstBuilder<'a>` or `&mut TraverseCtx<'a>`. The arena lifetime `'a` propagates through every helper in the call chain.

Keep these categories separate to avoid unnecessary lifetime infection.

## Naming

- **Modules:** Snake case matching SWC names where applicable (`collector.rs`, `entry_strategy.rs`, `code_move.rs`)
- **Types:** PascalCase (`TransformPlan`, `SegmentPlan`, `SegmentAnalysis`)
- **Functions:** snake_case (`transform_modules`, `build_segment_program`)
- **Constants:** UPPER_SNAKE_CASE (`BUILDER_IO_QWIK`)

## Commits

Conventional commits:
- `feat:` — New transformation feature
- `fix:` — Bug fix
- `refactor:` — Code restructuring
- `test:` — Test changes
- `chore:` — Build, deps, scripts

## Identifiers in OXC

OXC has three identifier types (vs SWC's one):
- `BindingIdentifier` — Declaration sites (`const x`, `import { x }`)
- `IdentifierReference` — Usage sites (`console.log(x)`)
- `IdentifierName` — Property names (`obj.prop`)

Use the correct `AstBuilder` method for each: `ast.binding_identifier()`, `ast.expression_identifier_reference()`, `ast.identifier_name()`.
