# Integrations

## OXC Pipeline

The optimizer chains these OXC subsystems:

```
oxc_parser::Parser → Program<'a>
    ↓
oxc_semantic::SemanticBuilder → Scoping (scope tree, symbol table, references)
    ↓
oxc_traverse::traverse_mut → Traverse trait (enter_*/exit_* callbacks)
    ↓ (ctx.ast = AstBuilder for node construction)
oxc_codegen::Codegen → CodegenReturn { code, map }
```

### Key APIs

| Subsystem | Key API | Used For |
|-----------|---------|----------|
| `oxc_parser` | `Parser::new(&allocator, source, source_type).parse()` | Parse input to AST |
| `oxc_semantic` | `SemanticBuilder::new().build(&program)` | Scope/symbol analysis |
| `oxc_semantic` | `Scoping::find_binding()`, `scope_ancestors()`, `get_resolved_references()` | Capture analysis |
| `oxc_traverse` | `traverse_mut(&mut visitor, &allocator, &mut program, ...)` | AST traversal with mutation |
| `oxc_ast` | `ctx.ast.*` (AstBuilder) | Construct new AST nodes |
| `oxc_codegen` | `Codegen::new().with_source_text(src).build(&program)` | Generate JS + source maps |
| `oxc_sourcemap` | `SourceMap::to_json_string()` | Serialize source maps |

## Qwik Runtime

The optimizer produces output consumed by the Qwik runtime:

- **QRL wrappers:** `qrl(() => import("./segment"), "symbolName")` — lazy import + symbol name
- **Segment modules:** Standalone files with `export const SymbolName = ...`
- **Segment metadata:** JSON with `origin`, `name`, `hash`, `captures`, `ctxKind`, `ctxName`, etc.
- **Diagnostics:** Errors/warnings about invalid `$()` usage, missing captures, etc.

## Build Tools

The optimizer is called by Vite/Rollup plugins (in `swc-optimizer/src/plugins/`). The new OXC optimizer is a drop-in replacement — same `TransformModulesOptions` input, same `TransformOutput` output. The plugin layer is NOT being rewritten; only the Rust core.

## Test Infrastructure

- **insta** — Snapshot testing crate. `insta::assert_snapshot!` compares output strings.
- **oxfmt** — Code formatter. The test harness auto-formats code blocks before snapshot comparison.
- **regex-lite** — Hash placeholder replacement in snapshots.
