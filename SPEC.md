# Qwik Optimizer Behavioral Specification

This document is a **behavioral specification** of every transform, rewrite, and code generation behavior the Qwik Rust/SWC optimizer performs.

**Purpose:** This spec is precise enough for the OXC team to reimplement the optimizer from scratch without reading the Rust source. Every claim is derived directly from the Rust source in `packages/optimizer/core/src/`. The Rust source is the sole source of truth — existing documentation may be stale and should not be relied upon.

**Structure:**

*   **Chapter 1 (this document):** Vocabulary (Glossary) and Public API types
    
*   **Chapter 2:** Core `QwikTransform` — segment extraction, QRL generation, call form details
    
*   **Chapter 3:** Inline / Hoist mode and `_fnSignal` inlining
    
*   **Chapter 4:** JSX transforms (`_jsxSplit`, props destructuring)
    
*   **Chapter 5:** Dependency analysis and variable migration
    

* * *

## Glossary

The following terms have precise meanings within the optimizer. Each definition includes a Rust source reference so an implementor can verify the claim.

### Segment

A **segment** is a closure extracted from the source file by the optimizer and emitted as a standalone JavaScript module. The closure must be the argument to a **marker function** (a function whose name ends with `$`, e.g. `component$`, `useTask$`).

Every segment has five properties:

| Property | Rust field | Description |
| --- | --- | --- |
| Symbol name | `Segment.name` | Stable identifier string — used in QRL calls and the manifest key |
| Display name | `SegmentData.display_name` | Human-readable, file-prefixed name — present in non-Prod modes |
| Canonical filename | `Segment.canonical_filename` | Output filename stem: `{display_name}_{hash_suffix}` |
| Entry | `Segment.entry` | Optional bundle grouping key determined by EntryStrategy |
| Captures | `SegmentData.captures` | `Captures` enum — either `Auto(Vec<Id>)` (optimizer-inferred) or `Explicit(ArrayLit)` (user-provided). The boolean `captures` on `SegmentAnalysis` (parse.rs:61) is derived: `true` when `scoped_idents` is non-empty |

Source: `Segment` struct (`name`, `entry`, `canonical_filename`) and `SegmentData` struct (`display_name`, `captures`) in `packages/optimizer/core/src/transform.rs` lines 54–88. `SegmentAnalysis` struct in `packages/optimizer/core/src/parse.rs` lines 49–67.

### QRL (Qwik Resource Locator)

A **QRL** is a lazy reference to an extracted segment. At call sites in the transformed output, a `$`\-suffixed call is replaced by a `qrl()`/`inlinedQrl()`/`_noopQrl()` call that references the segment by its import path and symbol name. The QRL allows the Qwik runtime to lazily load the segment's module on demand.

There are four QRL call forms (full call form details in Chapter 2):

| Form | Used when |
| --- | --- |
| `qrl(chunkPath, symbolName, captures)` | Prod and Test modes — extracted segments |
| `qrlDEV(chunkPath, symbolName, captures, devInfo)` | Dev and Hmr modes — extracted segments with source location |
| `inlinedQrl(fn, symbolName, captures)` | Inline, Hoist, and Lib modes — closure inlined, not extracted |
| `_noopQrl()` / `_noopQrlDEV()` | Server-only segments stripped at build time |

Source: `QwikTransform` in `packages/optimizer/core/src/transform.rs`; call form selection in `packages/optimizer/core/src/parse.rs`.

### Capture

A **capture** is a variable from an enclosing lexical scope that a segment closure references. Captures travel in two distinct lists that serve different purposes:

`scoped_idents` **— Runtime capture array (passed via** `_captures` **parameter)**

Variables that must be passed to the segment at runtime. When the segment executes, these are supplied in the QRL's `_captures` argument. When `scoped_idents` is non-empty, `captures: true` in `SegmentAnalysis`.

`local_idents` **— Compile-time dependency list (become** `import` **statements)**

Identifiers that drive `import` statement generation in the extracted segment module file. These do NOT require runtime passing — they are statically imported into the segment's standalone module file.

**Critical distinction:** A variable CAN appear in `local_idents` without being in `scoped_idents`. For example, a constant imported from another module is a compile-time dependency (imported in the segment file) but does not need to be captured at runtime via the `_captures` array.

Source: `SegmentData` fields `local_idents` and `scoped_idents` in `packages/optimizer/core/src/transform.rs` lines 75–77.

### Symbol Name

The **symbol name** is the stable identifier string for a segment. It is used in QRL calls and as the manifest key. The format depends on `EmitMode`:

| EmitMode | Symbol name format |
| --- | --- |
| `Prod` | `s_{hash64}` — hash only, no human-readable prefix |
| `Dev`, `Hmr`, `Lib`, `Test` | `{display_name}_{hash64}` — human-readable prefix plus hash |

Note: In Prod mode the `s_` prefix is a fixed literal — it is not an abbreviation of the display name.

Source: `register_context_name` in `packages/optimizer/core/src/transform.rs` lines 407–414.

### Display Name

The **display name** is the human-readable name component for a segment. It is composed by the following 5-step algorithm:

**Step 1:** Join `stack_ctxt` entries with `_`.

`stack_ctxt` is a stack of component/function names accumulated as the optimizer walks the AST. For a handler inside `<Counter>`, the stack might be `["Counter", "handleClick"]`.

**Step 2:** Apply `escape_sym` to the joined string.

The `escape_sym` algorithm:

*   Keep characters matching `[A-Za-z0-9]` unchanged
    
*   Replace any other character with `_`
    
*   Squash consecutive `_` into a single `_`
    
*   Trim leading and trailing `_`
    

**Step 3:** Prepend `_` if the first character is a digit.

This ensures the result is a valid JavaScript identifier.

**Step 4:** Append `_{N}` collision suffix if a collision counter > 0.

The optimizer tracks a `segment_names: HashMap<String, u32>` per file. If the same display name has been used before in this file, the counter is incremented and appended (e.g. `handler_1`, `handler_2`).

**Step 5:** Prepend `{file_name}_` to form the final display name stored in `SegmentAnalysis`.

`file_name` is the source file's stem with extension, escaped through `escape_sym` (e.g. `component_tsx` for `Component.tsx`).

Example: For a `handleClick$` inside `Counter` in `counter.tsx`, the display name would be `counter_tsx_Counter_handleClick`.

Source: `register_context_name` in `packages/optimizer/core/src/transform.rs` lines 363–421.

### Hash

The **hash** is a deterministic 11-character base64url string that uniquely identifies a segment. It is computed from `(scope, rel_path, display_name)` using the following 5-step algorithm:

**Step 1:** Create `DefaultHasher::new()`.

**Step 2:** If `scope` is `Some(s)`, call `hasher.write(s.as_bytes())`.

`scope` comes from `TransformModulesOptions.scope` — it is a namespace prefix that prevents symbol collisions across packages.

**Step 3:** Call `hasher.write(local_file_name.as_bytes())`.

`local_file_name` is the slash-normalized relative file path from `src_dir`.

**Step 4:** Call `hasher.write(display_name.as_bytes())`.

This is the pre-file-prefix display name (Steps 1–4 of the Display Name algorithm above — before Step 5 prepends `{file_name}_`).

**Step 5:** `hasher.finish()` → `u64` → little-endian bytes → `URL_SAFE_NO_PAD` base64 → replace `-` and `_` with `0`.

The verbatim `base64()` function from the source:

```rust
fn base64(data: &[u8]) -> String {
    BASE64_URL_SAFE_NO_PAD
        .encode(data)
        .replace(['-', '_'], "0")
}
```

The replace step is non-standard — if omitted, all symbol names containing `-` or `_` in the hash will diverge from the reference implementation.

Source: `register_context_name` lines 393–419 and `base64()` function at line 4725–4729 in `packages/optimizer/core/src/transform.rs`.

### Canonical Filename

The **canonical filename** is the output filename stem (without extension) for the segment's standalone module file. Format:

```plaintext
{display_name}_{hash_suffix}
```

where `hash_suffix` is the **last** `_`**\-delimited token** of the `symbol_name`.

Concretely, this means:

*   In Prod mode: `symbol_name = "s_AbCdEfGhIjK"` → `hash_suffix = "AbCdEfGhIjK"` → `canonical_filename = "counter_tsx_Counter_click_AbCdEfGhIjK"`
    
*   In Dev mode: `symbol_name = "counter_tsx_Counter_click_AbCdEfGhIjK"` → `hash_suffix = "AbCdEfGhIjK"` → same canonical filename
    

The canonical filename is identical across all modes for the same segment.

Source: `get_canonical_filename` in `packages/optimizer/core/src/transform.rs` lines 4910–4913.

* * *

### Supporting Terms

**Marker Function**

A function whose name ends with `$` (e.g. `component$`, `useTask$`, `useVisibleTask$`, `onClick$`). The optimizer detects calls to marker functions and extracts their closure argument as a segment. The marker function name is recorded in `SegmentAnalysis.ctx_name`.

**SegmentKind**

Enum with three variants classifying the kind of marker function:

```rust
pub enum SegmentKind {
    Function,      // component$, useTask$, useVisibleTask$, etc.
    EventHandler,  // onClick$, onInput$, etc.
    JSXProp,       // inline JSX prop $-expressions
}
```

The distinction matters for `EntryStrategy::Smart` grouping: event handlers with no scope captures are given their own entry chunk regardless of component, because they are pure and always safe to share.

Source: `SegmentKind` in `packages/optimizer/core/src/transform.rs` lines 46–51.

**MinifyMode**

Enum controlling whether the SWC simplifier (DCE/constant folding) runs after segment extraction:

```rust
pub enum MinifyMode {
    Simplify,  // Run SWC simplifier (DCE/constant folding)
    None,      // Skip simplifier entirely
}
```

Note: `MinifyMode` has no effect in `EmitMode::Lib` — the entire post-processing block (including the simplifier) is skipped in Lib mode regardless of this setting.

Source: `MinifyMode` in `packages/optimizer/core/src/parse.rs` lines 69–74.

* * *

## Public API

The optimizer exposes two entry points:

*   `transform_modules` — Batch transform (primary API). Takes `TransformModulesOptions` containing a `Vec<TransformModuleInput>` and returns `TransformOutput`. This is the function called from WASM and NAPI bindings.
    
*   `transform_code` — Single-file transform (internal). Takes `TransformCodeOptions` for a single file. Called by `transform_modules` for each input file. Not directly exposed at the WASM/NAPI boundary.
    

All types are defined in `packages/optimizer/core/src/lib.rs` and `packages/optimizer/core/src/parse.rs`.

* * *

### TransformModulesOptions

The top-level configuration struct passed to `transform_modules()`.

Source: `TransformModulesOptions` in `packages/optimizer/core/src/lib.rs` lines 54–74.

| Field | Type | Semantics |
| --- | --- | --- |
| `src_dir` | `String` | Absolute path to the source root — used as the base for all relative path computation |
| `root_dir` | `Option<String>` | Optional project root — used for source map `sourceRoot` and manifest path relativization |
| `input` | `Vec<TransformModuleInput>` | List of source files to transform in this batch |
| `source_maps` | `bool` | When `true`, emit JSON source maps in each `TransformModule.map` |
| `minify` | `MinifyMode` | `Simplify` runs SWC simplifier (DCE/constant folding); `None` skips it |
| `transpile_ts` | `bool` | When `true`, strip TypeScript syntax via SWC TS stripper |
| `transpile_jsx` | `bool` | When `true`, convert JSX to `_jsx`/`_jsxs` calls via SWC React transform |
| `preserve_filenames` | `bool` | When `true`, output path retains the original file extension instead of `.js` |
| `entry_strategy` | `EntryStrategy` | Chunking strategy — determines which segments are grouped into the same bundle |
| `explicit_extensions` | `bool` | When `true`, append file extension in QRL import paths |
| `mode` | `EmitMode` | Output contract — governs QRL call form, segment file emission, and const replacement |
| `scope` | `Option<String>` | Namespace prefix for hash computation — prevents symbol name collisions across packages. Included as first input to the hash (see Glossary > Hash). |
| `core_module` | `Option<String>` | Override the `@qwik.dev/core` import source. Defaults to `@qwik.dev/core` (via the `BUILDER_IO_QWIK` constant). |
| `strip_exports` | `Option<Vec<Atom>>` | Named exports to remove from the source before any other transform |
| `strip_ctx_name` | `Option<Vec<Atom>>` | Marker function names to strip — their closure arguments are replaced with `_noopQrl()` instead of being extracted |
| `strip_event_handlers` | `bool` | When `true`, strip all event handler segments (replace with `_noopQrl()`) |
| `reg_ctx_name` | `Option<Vec<Atom>>` | Marker function names that register symbols (in addition to the built-in defaults) |
| `is_server` | `Option<bool>` | Whether this is a server build. **Defaults to** `true` **when** `None` — not `false`. This governs `isServer`/`isBrowser` const replacement values. |

\*\*\`is\_server\` gotcha:\*\* The default when \`None\` is \`true\`, not \`false\`. A caller that omits \`is\_server\` gets server-build behavior. To get client behavior, the caller must explicitly pass \`is\_server: Some(false)\`.

* * *

### TransformModuleInput

Each entry in `TransformModulesOptions.input`.

Source: `TransformModuleInput` in `packages/optimizer/core/src/lib.rs` lines 46–50.

| Field | Type | Semantics |
| --- | --- | --- |
| `path` | `String` | Relative path from `src_dir` — used for symbol naming and output module path |
| `dev_path` | `Option<String>` | Override path shown in Dev/Hmr QRL dev-info objects. When `None`, `path` is used. |
| `code` | `String` | UTF-8 source text of the module |

* * *

### TransformCodeOptions

The per-file configuration struct used internally by `transform_code()`. Not directly exposed at the WASM/NAPI boundary — `transform_modules` constructs one of these from `TransformModulesOptions` for each input file.

Source: `TransformCodeOptions` in `packages/optimizer/core/src/parse.rs` lines 86–109.

| Field | Type | Semantics |
| --- | --- | --- |
| `relative_path` | `&str` | Relative path from `src_dir` |
| `dev_path` | `Option<&str>` | Dev-mode path override (from `TransformModuleInput.dev_path`) |
| `src_dir` | `&Path` | Absolute source root (from `TransformModulesOptions.src_dir`) |
| `root_dir` | `Option<&Path>` | Optional project root |
| `source_maps` | `bool` | Emit source maps |
| `minify` | `MinifyMode` | `Simplify` or `None` |
| `transpile_ts` | `bool` | Strip TypeScript |
| `transpile_jsx` | `bool` | Transform JSX |
| `preserve_filenames` | `bool` | Keep original extension in output path |
| `explicit_extensions` | `bool` | Append extension in QRL import paths |
| `code` | `&str` | Source text |
| `entry_policy` | `&dyn EntryPolicy` | Resolved strategy object — produced by `parse_entry_strategy(entry_strategy)` |
| `mode` | `EmitMode` | Output contract |
| `scope` | `Option<&String>` | Hash namespace scope |
| `entry_strategy` | `EntryStrategy` | Raw strategy variant — needed for the Inline/Hoist branch check (`is_inline()`) |
| `core_module` | `Atom` | Resolved core module specifier |
| `reg_ctx_name` | `Option<&[Atom]>` | Marker function names to register |
| `strip_exports` | `Option<&[Atom]>` | Named exports to remove |
| `strip_ctx_name` | `Option<&[Atom]>` | Marker names to replace with noop |
| `strip_event_handlers` | `bool` | Strip all event handler segments |
| `is_server` | `bool` | Server build flag (resolved — never `None` at this level) |

* * *

### TransformOutput

The return value of both `transform_modules()` and `transform_code()`.

Source: `TransformOutput` in `packages/optimizer/core/src/parse.rs` lines 111–118; `get_manifest` lines 149–178.

| Field | Type | Semantics |
| --- | --- | --- |
| `modules` | `Vec<TransformModule>` | All output modules — one per input file (parent module) plus one per extracted segment |
| `diagnostics` | `Vec<Diagnostic>` | Errors and warnings produced during the transform |
| `is_type_script` | `bool` | `true` if any input module was detected as TypeScript |
| `is_jsx` | `bool` | `true` if any input module was detected as JSX |

`get_manifest()` **method:** Builds a `QwikManifest` from all segment modules in `modules`. Iterates over modules, skips entries where `segment` is `None` (i.e. parent modules), and collects:

*   `symbols`: `segment.name → SegmentAnalysis`
    
*   `bundles`: `canonical_filename.extension → QwikBundle { size, symbols }`
    
*   `mapping`: `segment.name → canonical_filename.extension`
    

* * *

### TransformModule

Represents a single output module in `TransformOutput.modules`. There is one parent module per input file (with `segment: None`) and one segment module per extracted closure (with `segment: Some(...)`).

Source: `TransformModule` in `packages/optimizer/core/src/parse.rs` lines 181–194.

| Field | Type | Serialized | Semantics |
| --- | --- | --- | --- |
| `path` | `String` | yes | Output relative path (from `src_dir`) |
| `code` | `String` | yes | Generated JavaScript/TypeScript text |
| `map` | `Option<String>` | yes | JSON source map string, present when `source_maps: true` |
| `segment` | `Option<SegmentAnalysis>` | yes | Populated only for extracted segment modules; `None` for parent modules |
| `is_entry` | `bool` | yes | `true` when the segment's `entry` is `None` (i.e. this segment is its own entry chunk) |
| `order` | `u64` | **no** | Deterministic sort key based on segment hash (or path hash for parent modules). **Not emitted to JSON.** |

\`TransformModule.order\` is present in the Rust struct but skipped during serialization. Consumers sorting by this field from deserialized JSON will see a default value of \`0\` for all modules.

* * *

### SegmentAnalysis

The per-segment metadata included in `TransformModule.segment` and `QwikManifest.symbols`. This is the richest type in the API.

Source: `SegmentAnalysis` in `packages/optimizer/core/src/parse.rs` lines 47–67.

| Field | Type | Semantics |
| --- | --- | --- |
| `origin` | `Atom` | Relative path of the source file that produced this segment (e.g. `src/components/counter.tsx`) |
| `name` | `Atom` | Symbol name — the manifest key and QRL reference (e.g. `s_AbCdEfGhIjK` in Prod, `counter_tsx_Counter_click_AbCd` in Dev) |
| `entry` | `Option<Atom>` | Bundle grouping key — `None` means this segment is its own entry chunk |
| `display_name` | `Atom` | File-prefixed human-readable name (e.g. `counter_tsx_Counter_click`) |
| `hash` | `Atom` | 11-character base64url hash string |
| `canonical_filename` | `Atom` | Output filename stem without extension (e.g. `counter_tsx_Counter_click_AbCdEfGhIjK`) |
| `path` | `Atom` | Subdirectory path within `src_dir` — usually the parent directory of `origin` |
| `extension` | `Atom` | Output file extension determined by `transpile_ts` and `transpile_jsx` flags |
| `parent` | `Option<Atom>` | Symbol name of the parent segment, if this segment is nested inside another |
| `ctx_kind` | `SegmentKind` | `Function`, `EventHandler`, or `JSXProp` |
| `ctx_name` | `Atom` | Name of the marker function that created this segment (e.g. `component$`, `useTask$`) |
| `captures` | `bool` | `true` when `scoped_idents` is non-empty (runtime captures exist) |
| `loc` | `(u32, u32)` | `(span.lo.0, span.hi.0)` — SWC byte offsets in the original source file |
| `param_names` | `Option<Vec<Atom>>` | Parameter identifiers if the segment is a function with declared parameters |
| `capture_names` | `Option<Vec<Atom>>` | Names of `scoped_idents` — only present when `captures: true` |

* * *

### QwikManifest

The minimal manifest produced by `TransformOutput::get_manifest()`.

Source: `QwikManifest` and `get_manifest` in `packages/optimizer/core/src/parse.rs` lines 127–178.

| Field | Type | Semantics |
| --- | --- | --- |
| `version` | `Atom` | Always `"1"` |
| `symbols` | `HashMap<Atom, SegmentAnalysis>` | Symbol name → segment analysis — one entry per extracted segment |
| `bundles` | `HashMap<Atom, QwikBundle>` | Canonical filename with extension → bundle info |
| `mapping` | `HashMap<Atom, Atom>` | Symbol name → canonical filename with extension |

\*\*Rust vs. Vite manifest:\*\* The \`QwikManifest\` in \`packages/qwik-vite/src/types.ts\` is a superset of what the Rust optimizer produces. The Vite plugin extends it post-transform with additional fields: \`manifestHash\`, \`bundleGraph\`, \`assets\`, and others. The Rust optimizer only produces the four fields listed above.

* * *

### QwikBundle

Represents a single output bundle in `QwikManifest.bundles`.

Source: `QwikBundle` in `packages/optimizer/core/src/parse.rs` lines 121–125.

| Field | Type | Semantics |
| --- | --- | --- |
| `size` | `usize` | Byte length of the generated module code |
| `symbols` | `Vec<Atom>` | Symbol names contained in this bundle — currently always exactly one per bundle |

* * *

### Diagnostic

Represents an error or warning produced during the transform.

Source: `Diagnostic` in `packages/optimizer/core/src/utils.rs` lines 45–54.

| Field | Type | Semantics |
| --- | --- | --- |
| `category` | `DiagnosticCategory` | `Error`, `Warning`, or `SourceError` |
| `code` | `Option<String>` | Formatted as `C{NN}` (e.g. `C02`, `C03`, `C05`) |
| `file` | `Atom` | Relative path of the source file where the diagnostic originated |
| `message` | `String` | Human-readable error or warning text |
| `highlights` | `Option<Vec<SourceLocation>>` | Source locations highlighted in the diagnostic output |
| `suggestions` | `Option<Vec<String>>` | Suggested fixes or next steps |
| `scope` | `DiagnosticScope` | Always `Optimizer` for optimizer-produced diagnostics |

`DiagnosticCategory` variants:

| Variant | Meaning |
| --- | --- |
| `Error` | Fatal — transform cannot proceed |
| `Warning` | Non-fatal — transform continues |
| `SourceError` | Error with source location context |

* * *

### Output Module Naming

**Parent module path** (the transformed input file):

```plaintext
{rel_dir}/{file_stem}.{extension}
```

When `preserve_filenames: true`:

```plaintext
{file_name}
```

where `file_name` is the original filename including its original extension.

**Segment module path** (each extracted closure):

```plaintext
{segment.path}/{canonical_filename}.{extension}
```

where:

*   `segment.path` is the subdirectory within `src_dir` (same as parent directory of `origin`)
    
*   `canonical_filename` is `{display_name}_{hash_suffix}` (see Glossary > Canonical Filename)
    
*   `extension` is determined by `transpile_ts` and `transpile_jsx` flags (`.js`, `.ts`, `.jsx`, `.tsx`)
    

* * *

## EmitMode

`EmitMode` governs the output contract for the entire transform. It determines QRL call forms, symbol name format, which post-processing passes run, and whether segment files are emitted. There are five variants defined in `packages/optimizer/core/src/parse.rs` lines 76–84.

The mode affects every segment produced by the transform. A single transform call uses one mode for all input files.

### Summary Comparison Table

| Behavior | Prod | Dev | Hmr | Lib | Test |
| --- | --- | --- | --- | --- | --- |
| Symbol name format | `s_{hash}` | `{display}_{hash}` | `{display}_{hash}` | `{display}_{hash}` | `{display}_{hash}` |
| QRL call form | `qrl()` | `qrlDEV()` | `qrlDEV()` | `inlinedQrl()` | `qrl()` |
| Const replacement | Yes | Yes | Yes | **No** | **No** |
| Segment files emitted | Yes | Yes | Yes | **No** | Yes |
| DCE/treeshaker | Yes | Yes | Yes | **No** | Yes |
| Dev info in QRL | No | Yes | Yes | No | No |
| Variable migration | Yes | Yes | Yes | **No** | Yes |

### EmitMode::Prod

**Use case:** Production client/server builds.

**Behavioral contract:**

*   **Symbol names:** `s_{hash64}` — hash-only format, no human-readable prefix. The `s_` prefix is a fixed literal (not an abbreviation of the display name).
    
*   **QRL call form:** `qrl(chunkPath, symbolName, captures)` — extracted segments, no dev info.
    
*   **Const replacement:** Runs — `isServer`, `isBrowser`, `isDev` are replaced with boolean literals by `ConstReplacerVisitor`.
    
*   **DCE/treeshaker:** Runs (if `minify != None`).
    
*   **Segment files:** Emitted as standalone modules.
    
*   **Dev info:** NOT included in QRL calls.
    

Source: `parse.rs` line 293 (`is_dev = matches!(EmitMode::Dev | EmitMode::Hmr)`), line 306 (`mode != EmitMode::Lib` const replacement gate), line 407 (symbol format).

### EmitMode::Dev

**Use case:** Development builds with enhanced debugging.

**Behavioral contract:**

*   **Symbol names:** `{display_name}_{hash64}` — human-readable prefix plus hash.
    
*   **QRL call form:** `qrlDEV(chunkPath, symbolName, captures, {fileName, lineNumber, columnNumber})` — with source location dev info object.
    
*   **Const replacement:** Runs — same as Prod.
    
*   **Segment files:** Emitted as standalone modules.
    
*   **Dev imports:** `qrlDEV`, `inlinedQrlDEV`, `_noopQrlDEV` are explicitly injected as imports into segment modules.
    

Source: `parse.rs` line 293 and lines 464–497 (dev import injection).

### EmitMode::Hmr

**Use case:** Hot module replacement mode during development.

**Behavioral contract:**

*   **Symbol names:** `{display_name}_{hash64}` — identical to Dev.
    
*   **QRL call form:** `qrlDEV(...)` — identical to Dev.
    
*   **Const replacement:** Runs — identical to Dev.
    
*   **Segment files:** Emitted as standalone modules.
    
*   **Dev imports:** Same explicit injection as Dev mode.
    
*   **Additional:** `_useHmr` hook injection for HMR component tracking (word defined in `words.rs` line 53).
    

The `is_dev` check at `parse.rs` line 293 (`matches!(EmitMode::Dev | EmitMode::Hmr)`) matches both Dev and Hmr — they are treated identically except for the `_useHmr` injection.

Source: `parse.rs` line 293; `words.rs` line 53.

### EmitMode::Lib

**Use case:** Pre-compiling library code for npm distribution.

\`EmitMode::Lib\` is a \*\*structurally distinct output path\*\*, not a flag variation on Prod or Dev. Two conditional gates in \`parse.rs\` (lines 306 and 351) redirect the entire post-processing flow. An implementor must treat Lib as a separate code path, not a configuration difference.

**Behavioral contract:**

*   **Symbol names:** `{display_name}_{hash64}` — human-readable, same as Dev.
    
*   **QRL call form:** `inlinedQrl(fn, symbolName, captures)` — the closure is **inlined** in the call, NOT extracted to a separate file.
    
*   **Const replacement:** **SKIPPED** — `if config.mode != EmitMode::Lib` gates `ConstReplacerVisitor` (`parse.rs` line 306).
    
*   **Segment files:** **NOT EMITTED** — `self.segments.push(...)` is never called; the entire post-processing block is skipped (`parse.rs` line 351).
    
*   **DCE/treeshaker:** **NOT RUN** — same `mode != EmitMode::Lib` gate.
    
*   **Variable migration:** **NOT RUN** — same gate.
    
*   **Props destructuring:** DOES run — this pass applies to all modes including Lib (`parse.rs` lines 296–303).
    

**Key implication:** Lib-mode output is a single module with `inlinedQrl` calls. There are no companion `*.js` segment files. The consuming app's optimizer is responsible for re-extracting segments when it processes the library.

The two conditional gates from the Rust source:

```rust
// Source: packages/optimizer/core/src/parse.rs lines 306–313
if config.mode != EmitMode::Lib {
    // replace const values (isServer, isBrowser, isDev)
    if config.mode != EmitMode::Test {
        let mut const_replacer =
            ConstReplacerVisitor::new(config.is_server, is_dev, &collect);
        program.visit_mut_with(&mut const_replacer);
    }
}

// Source: parse.rs lines 350–436
// Skip post-processing for library mode
if config.mode != EmitMode::Lib {
    // ... treeshaker, simplifier, SideEffectVisitor, variable migration
    segments = qwik_transform.segments.clone();
    qt = Some(qwik_transform);
    // ... variable migration
}
// → for Lib mode: segments is empty, no segment files are emitted
```

Source: `parse.rs` lines 306, 351, 396; `transform.rs` lines 407–414.

### EmitMode::Test

**Use case:** Unit testing the optimizer itself.

**Behavioral contract:**

*   **Symbol names:** `{display_name}_{hash64}` — human-readable, same as Dev/Lib.
    
*   **QRL call form:** Extracted `qrl()` — same as Prod (no dev info).
    
*   **Const replacement:** **SKIPPED** — `if config.mode != EmitMode::Test` gates `ConstReplacerVisitor` (`parse.rs` line 308). Note: the Lib gate (line 306) is the outer check; the Test gate (line 308) is the inner check within the Lib branch.
    
*   **Segment files:** Emitted — same as Prod/Dev/Hmr.
    
*   **Post-processing:** Runs — DCE, treeshaker, and variable migration all run.
    

Source: `parse.rs` line 308 (`mode != EmitMode::Test`).

* * *

## EntryStrategy

`EntryStrategy` determines the `entry: Option<Atom>` field on each segment. When `entry` is `Some(key)`, segments sharing the same key are bundled together by the downstream bundler into a single chunk. When `entry` is `None`, each segment is its own entry chunk (a separate lazy-loadable file).

All seven variants are defined in `packages/optimizer/core/src/entry_strategy.rs` lines 14–22.

### The EntryPolicy Trait

Each strategy variant maps to an `EntryPolicy` implementation via `parse_entry_strategy()`. The trait has one method:

```rust
fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<Atom>;
```

*   `context` — the component name stack at the segment's call site (e.g. `["Counter", "onClick"]`)
    
*   `segment` — the `SegmentData` for the segment being assigned an entry key
    
*   Returns `Some(key)` to group this segment with others sharing the same key, or `None` for its own chunk
    

### Summary Table

| Strategy | Policy Implementation | Grouping Rule |
| --- | --- | --- |
| Inline | InlineStrategy | All segments → `Some("entry_segments")` |
| Hoist | InlineStrategy | All segments → `Some("entry_segments")` |
| Single | SingleStrategy | All segments → `Some("entry_segments")` |
| Hook | PerSegmentStrategy | All segments → `None` (each own chunk) |
| Segment | PerSegmentStrategy | All segments → `None` (each own chunk) |
| Component | PerComponentStrategy | Per-component grouping by root name |
| Smart | SmartStrategy | Heuristic: pure handlers own chunk, rest per-component |

### EntryStrategy::Inline

**Policy:** `InlineStrategy` — `get_entry_for_sym` always returns `Some("entry_segments")`.

All segments share a single group key. The bundler places them all in one chunk.

Also triggers `is_inline()` to return `true`, which gates the `SideEffectVisitor` pass (`parse.rs` lines 371–379). This pass removes unused side effects and only runs for Inline and Hoist strategies.

**Output shape distinction from Hoist:** Both Inline and Hoist use `InlineStrategy` for grouping. The difference is purely in output module structure — Inline uses inline QRL call expressions; Hoist hoists QRL calls to module scope as `const q_ = qrl(...)` declarations. See Chapter 3 (Phase 12) for output shape details.

Source: `entry_strategy.rs` lines 29–35 and 117.

### EntryStrategy::Hoist

**Policy:** `InlineStrategy` — identical to Inline (`get_entry_for_sym` returns `Some("entry_segments")`).

Same grouping as Inline. Also triggers `is_inline()` to return `true` (same as Inline).

Difference from Inline is only in output module structure: QRL calls are hoisted to module scope as `const q_ = qrl(...)` declarations instead of inline call expressions. See Chapter 3 (Phase 12) for output shape details.

Source: `entry_strategy.rs` line 117.

### EntryStrategy::Single

**Policy:** `SingleStrategy` — `get_entry_for_sym` always returns `Some("entry_segments")`.

All segments go into a single bundle entry named `"entry_segments"`. Semantically same as Inline/Hoist at the grouping level.

**Distinction from Inline/Hoist:** Single does NOT trigger `is_inline()` — it is the only strategy that produces the `"entry_segments"` key without triggering the inline branch. `SideEffectVisitor` does NOT run for Single strategy.

Source: `entry_strategy.rs` lines 38–50.

### EntryStrategy::Hook

**Policy:** `PerSegmentStrategy` — `get_entry_for_sym` always returns `None`.

Each segment is its own entry chunk — no grouping. Every segment becomes a separate lazy-loadable file.

**Deprecated name:** `Hook` is the older API name; `Segment` is the current preferred name. They are functionally identical (see below).

Source: `entry_strategy.rs` lines 53–65.

### EntryStrategy::Segment

**Policy:** `PerSegmentStrategy` — identical to Hook (`get_entry_for_sym` returns `None`).

Functionally identical to Hook — both map to `PerSegmentStrategy`, both always return `None`. The only difference is the variant name at the API boundary.

Source: `entry_strategy.rs` lines 53–65 and 119.

### EntryStrategy::Component

**Policy:** `PerComponentStrategy`

Grouping rule:

*   If no component context (`context.first()` is `None`): returns `Some("entry_segments")` — top-level segments share one chunk.
    
*   Otherwise: returns `Some("{origin}_entry_{root_component_name}")` — per-component grouping where `root` is `context.first()`.
    

```rust
// Source: packages/optimizer/core/src/entry_strategy.rs lines 67–83
fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<Atom> {
    context.first().map_or_else(
        || Some(Atom::from("entry_segments")),
        |root| Some(Atom::from([&segment.origin, "_entry_", root].concat())),
    )
}
```

All segments from the same component (same `origin` + `root_component_name`) receive the same entry key and are bundled together. This is the most common production strategy.

Source: `entry_strategy.rs` lines 67–83.

### EntryStrategy::Smart

**Policy:** `SmartStrategy`

The Smart strategy applies a three-branch heuristic to balance loading efficiency:

1.  **Event handlers with no scope captures** — Own chunk (`None`): triggered when `scoped_idents.is_empty()` AND (`ctx_kind != Function` OR `ctx_name == "event$"`). These are pure and always safe to share across components.
    
2.  **Top-level QRLs** — Own chunk (`None`): triggered when `context.first()` is `None`. QRLs defined outside any component context are their own chunks.
    
3.  **All other segments** — Per-component grouping: `Some("{origin}_entry_{root_component_name}")` — same formula as the Component strategy.
    

**Intent:** Pure event handlers (branch 1) can be deduplicated across components because they have no captured state. Everything else (stateful handlers, lifecycle hooks, components) loads together per-component to minimize network round trips.

The verbatim Rust implementation:

```rust
// Source: packages/optimizer/core/src/entry_strategy.rs lines 96–112
fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<Atom> {
    // Event handlers without scope variables → own chunk
    if segment.scoped_idents.is_empty()
        && (segment.ctx_kind != SegmentKind::Function || &segment.ctx_name == "event$")
    {
        return None;
    }
    // Top-level QRLs → own chunk
    context.first().map_or_else(
        || None,
        // Per-component grouping
        |root| Some(Atom::from([&segment.origin, "_entry_", root].concat())),
    )
}
```

Source: `entry_strategy.rs` lines 86–113.

### is\_inline() Gate

Only `Inline` and `Hoist` strategies cause `is_inline()` to return `true`. This function is called at `parse.rs` lines 371–379 to gate the `SideEffectVisitor` pass, which removes unused side effects from the output.

```rust
fn is_inline(entry_strategy: &EntryStrategy) -> bool {
    matches!(entry_strategy, EntryStrategy::Inline | EntryStrategy::Hoist)
}
```

`Single` produces the same `"entry_segments"` grouping key as Inline and Hoist but does **not** trigger this gate — `SideEffectVisitor` does not run for Single strategy even though its output is grouped identically.

* * *

## Chapter 2: The Transform Pipeline

The entire per-file transform pipeline is defined in a single function `transform_code` in `packages/optimizer/core/src/parse.rs` (lines 205–650). It takes a source string and configuration, and produces a `TransformOutput`.

### Pipeline Stages

Each call to `transform_code` runs up to 13 stages in strict execution order. Stages are numbered 0–13 for reference; stages without a number range have no standalone source reference.

| Stage | Pass | Visitor Type | Conditional? | Condition |
| --- | --- | --- | --- | --- |
| 0 | Path parsing | `parse_path()` | No | Always |
| 1 | Source parsing | SWC `parse()` | No | Always |
| 2 | Strip exports | `StripExportsVisitor` | Yes | `config.strip_exports.is_some()` |
| 3 | TypeScript strip | `typescript::strip()` | Yes | `transpile_ts && is_type_script` |
| 4 | JSX transpile | `react::react()` | Yes | `transpile_jsx && is_jsx` |
| 5 | Import rename | `RenameTransform` | No | Always |
| 6 | Resolver | `resolver()` | No | Always |
| 7 | Global collect | `global_collect()` | No | Always |
| 8 | Props destructuring | `transform_props_destructuring()` | No | Always (all modes including Lib) |
| 9 | Const replacement | `ConstReplacerVisitor` | Yes | `mode != EmitMode::Lib && mode != EmitMode::Test` |
| 10 | QwikTransform (core) | `QwikTransform` fold | No | Always |
| 11 | Post-processing (DCE / treeshake / side-effects) | Multiple visitors | Yes | `mode != EmitMode::Lib` |
| 12 | Variable migration | `apply_variable_migration()` | Yes | `mode != EmitMode::Lib && !segments.is_empty()` |
| 13 | Hygiene + fixer | `hygiene_with_config`, `fixer` | No | Always |

The current \`build/v2\` branch has 13 stages. PR #8482 adds an \`EachTransform\` pass between Stage 4 (JSX transpile) and Stage 5 (import rename) when it lands. That pass rewrites \`.map()\` calls in JSX children to \`\` components. The spec documents the current 13-stage pipeline.

### Stage Dependencies

Six ordering constraints are non-negotiable. Swapping stages that share a dependency will silently corrupt results.

*   **Stage 6 → Stage 7:** Resolver must precede `global_collect`. The resolver assigns `SyntaxContext` hygiene marks that make identifiers unique; `global_collect` captures these marked identifiers as `Id = (Atom, SyntaxContext)`. Without resolved marks, all same-named identifiers collapse into a single entry.
    
*   **Stage 7 → Stage 8 and Stage 9:** `global_collect` must precede both `props_destructuring` and `const_replacement`. Both passes query the collect result to find their target identifiers by `Id`.
    
*   **Stage 8 → Stage 10:** Props destructuring must precede `QwikTransform`. `QwikTransform` sees already-rewritten `_rawProps` parameters; it relies on props having been unwrapped before it runs.
    
*   **Stage 3 → Stage 6 (when** `transpile_ts`**):** TypeScript strip must precede the resolver when TypeScript is being transpiled. The SWC resolver requires a clean JavaScript AST; TypeScript type annotations confuse identifier resolution.
    
*   **Stage 4 → Stage 6 (when** `transpile_jsx`**):** JSX transpile must precede the resolver when JSX is being transpiled. JSX is lowered to `jsx()` / `jsxDEV()` call expressions before identifier resolution runs, so the resolver can see the real identifiers.
    
*   **Stage 2 → Stage 3/4:** Strip exports must precede TypeScript and JSX transpilation. It operates on the raw parsed AST before any other transformation touches module structure.
    

### The `did_transform` Flag

After Stages 3 and 4, `transform_code` sets a `did_transform` boolean. The flag is `true` when TypeScript or JSX was actually transpiled (i.e., both the conditional and the detection flag were true).

This flag controls two output behaviors:

1.  **Output** `extension` **field:** When `did_transform` is `true` and `preserve_filenames` is `false`, the output extension is changed to `.js` (TypeScript → JavaScript, `.tsx` → `.jsx` → `.js`). When `preserve_filenames` is `true`, the original extension is kept regardless.
    
2.  `preserve_filenames` **interaction:** The `preserve_filenames` flag prevents the extension override. Library builds typically set this to preserve `.ts` / `.tsx` extensions in output so the consuming build can re-process them.
    

Source: `transform_code` in `packages/optimizer/core/src/parse.rs` lines 205–650.

### Pipeline Skeleton

The following skeleton shows the complete execution structure of `transform_code`. Omitted details (error handling, option construction) are present in the source but do not affect ordering.

```rust
// Source: transform_code in packages/optimizer/core/src/parse.rs lines 205–650

// Stage 0-1: Parse
let path_data = parse_path(config.relative_path, config.src_dir)?;
let (program, comments, is_type_script, is_jsx) = parse(config.code, &path_data, ...)?;

swc_common::GLOBALS.set(&Globals::new(), || {
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();

    // Stage 2: Strip exports (conditional)
    if let Some(strip_exports) = config.strip_exports {
        program.visit_mut_with(&mut StripExportsVisitor::new(strip_exports));
    }

    // Stage 3: TypeScript strip (conditional)
    if transpile_ts && is_type_script {
        program.mutate(&mut typescript::strip(Default::default(), top_level_mark));
        did_transform = true;
    }

    // Stage 4: JSX transpile (conditional)
    if transpile_jsx && is_jsx {
        program.mutate(&mut react::react(...));
        did_transform = true;
    }

    // Stage 5: Rename imports (always)
    program.visit_mut_with(&mut RenameTransform);

    // Stage 6: Resolver (always)
    program.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, ...));

    // Stage 7: Global collect (always)
    let mut collect = global_collect(&program);

    // Stage 8: Props destructuring (always, all modes)
    transform_props_destructuring(&mut program, &mut collect, &config.core_module);

    // Stage 9: Const replacement (Lib and Test modes skip)
    if config.mode != EmitMode::Lib && config.mode != EmitMode::Test {
        let mut const_replacer = ConstReplacerVisitor::new(config.is_server, is_dev, &collect);
        program.visit_mut_with(&mut const_replacer);
    }

    // Stage 10: Core QwikTransform (always)
    let mut qwik_transform = QwikTransform::new(QwikTransformOptions { ..collect.. });
    program = program.fold_with(&mut qwik_transform);

    // Stages 11-12: Post-processing (mode != Lib)
    // Stage 13: Hygiene + fixer (always)
    program.visit_mut_with(&mut hygiene_with_config(Default::default()));
    program.visit_mut_with(&mut fixer(None));
})
```

### GlobalCollect

`GlobalCollect` is the central data structure populated by Stage 7 and passed to all subsequent stages. It represents a complete read of the source file's import/export/declaration surface before any Qwik-specific transformation runs.

Source: `GlobalCollect` struct and `global_collect()` in `packages/optimizer/core/src/collector.rs`.

#### The `Id` Type

```rust
pub type Id = (Atom, SyntaxContext);
```

An `Id` is a pair of the identifier's symbol name and its SWC `SyntaxContext` (hygiene mark). Two identifiers that spell the same name but were introduced in different scopes receive different `SyntaxContext` values from the SWC resolver. This means that code like:

```js
import { foo } from 'a';
function outer() {
  const foo = 1; // different SyntaxContext — a different Id
}
```

produces two distinct `Id` values even though both spell `"foo"`.

This is why Stage 6 (resolver) **must** run before Stage 7 (global\_collect): without resolved hygiene marks, every same-named identifier would collapse to the same `Id`, making all four maps unreliable.

#### Structure

```rust
// Source: GlobalCollect struct in packages/optimizer/core/src/collector.rs
pub struct GlobalCollect {
    pub synthetic: Vec<(Id, Import)>,          // imports added during transform (not from source)
    pub imports: IndexMap<Id, Import>,          // all source-level imports keyed by local Id
    pub exports: IndexMap<Atom, ExportInfo>,   // all named exports keyed by exported symbol name
    pub root: IndexMap<Id, Span>,              // all top-level variable/function/class declarations

    rev_imports: HashMap<(Atom, Atom), Id>,    // (specifier, source) -> local Id (reverse lookup)
    canonical_ids: HashMap<Atom, Id>,          // first-seen Id for a given symbol name
    in_export_decl: bool,                      // visitor state flag
}
```

| Field | Type | Description |
| --- | --- | --- |
| `synthetic` | `Vec<(Id, Import)>` | Imports added during transform by `QwikTransform` or `props_destructuring`. Not from the original source. |
| `imports` | `IndexMap<Id, Import>` | All source-level imports keyed by local `Id`. Each entry maps the local binding to its `Import { source, specifier, kind, synthetic: false }`. |
| `exports` | `IndexMap<Atom, ExportInfo>` | All named exports keyed by exported symbol name (`Atom`). Includes re-exports, function exports, class exports, and the `"default"` key for default exports. |
| `root` | `IndexMap<Id, Span>` | All top-level variable, function, class, and enum declarations keyed by their `Id`. Import and export declarations are excluded — those are captured by `imports`/`exports`. |
| `rev_imports` | `HashMap<(Atom, Atom), Id>` | Reverse lookup: `(specifier, source) → local Id`. Used by `get_imported_local()` to find whether a particular import already exists. |
| `canonical_ids` | `HashMap<Atom, Id>` | Maps each symbol `Atom` to the first `Id` registered with that name. Used by `canonical_id_for()` when multiple identifiers share a name. |
| `in_export_decl` | `bool` | Visitor state flag. Set to `true` while visiting an export declaration so that `visit_binding_ident` / `visit_assign_pat_prop` know to populate `exports` instead of `root`. |

#### Population Logic

`global_collect(program)` constructs a `GlobalCollect` with all maps pre-allocated to capacity 16, then runs `program.visit_with(&mut collect)`. The `Visit` impl populates the maps via these six visitor methods:

```rust
// Source: global_collect in packages/optimizer/core/src/collector.rs line 56
pub fn global_collect(program: &ast::Program) -> GlobalCollect {
    let mut collect = GlobalCollect {
        synthetic: vec![],
        imports: IndexMap::with_capacity(16),
        exports: IndexMap::with_capacity(16),
        root: IndexMap::with_capacity(16),
        rev_imports: HashMap::with_capacity(16),
        canonical_ids: HashMap::with_capacity(16),
        in_export_decl: false,
    };
    program.visit_with(&mut collect);
    collect
}
// After this call: imports, exports, root are fully populated.
// Invariant: these three maps reflect only what the source file declares.
// synthetic is empty; rev_imports and canonical_ids are populated as side-effects.
```

The six visitor methods:

*   `visit_import_decl` — Populates `imports` for each named, default, and namespace specifier. Each entry maps `local Id → Import { source, specifier, kind, synthetic: false }`. Also populates `rev_imports` and `canonical_ids` as side-effects.
    
*   `visit_named_export` — Populates `exports` for `export { foo }` and `export { foo as bar }`. Only processes `node.src.is_none()` (local re-exports, not `export { foo } from 'other'`).
    
*   `visit_export_decl` — Populates `exports` for `export const`, `export function`, `export class`, and `export enum`. Sets `in_export_decl = true` before visiting children so nested binding visitors capture exports.
    
*   `visit_export_default_decl` — Populates `exports` with `"default"` key for named default class and function exports (e.g., `export default function Foo`).
    
*   `visit_module_item` — Populates `root` for top-level `function`, `class`, `var`, and `enum` declarations. Import and export declarations are excluded — those are handled by their dedicated visitors.
    
*   `visit_binding_ident` and `visit_assign_pat_prop` — Populate `exports` during `export const { a, b } = ...` destructuring. These fire for each destructured binding while `in_export_decl` is `true`.
    

#### Stage 7 Invariant

When \`global\_collect\` completes (end of Stage 7), the \`imports\`, \`exports\`, and \`root\` maps reflect the \*\*source file as-written\*\* — before any Qwik-specific transforms. This is the Stage 7 baseline. Subsequent stages query this baseline to find their target identifiers.

Three exceptions mutate `GlobalCollect` after Stage 7:

1.  `transform_props_destructuring` **(Stage 8):** Calls `global_collect.import()` to add a synthetic `_restProps` import when a component uses rest props (`...rest`). Additive only — no entries are removed.
    
2.  `QwikTransform` **(Stage 10):** Calls `global_collect.import()` to add synthetic imports for `qrl`, `qrlDEV`, `inlinedQrl`, and other runtime helpers as segments are extracted. Additive only.
    
3.  `apply_variable_migration` **(Stage 12):** Calls `global_collect.remove_root_and_exports_for_id()` to remove migrated variable declarations. This is the only destructive mutation — it occurs post-transform (Stage 12) and modifies the post-transform collect state, not the Stage 7 baseline.
    

An OXC reimplementor should treat the Stage 7 baseline as logically immutable and the `synthetic` list as the accumulator for additions.

#### Query Methods

| Method | Purpose |
| --- | --- |
| `get_imported_local(specifier, source)` | Returns the local `Id` for a given `(specifier, source)` pair if the import exists. Used by `props_destructuring` and `const_replace` to check for existing imports before adding synthetic ones. |
| `is_global(id)` | Returns `true` if the `id` is present in `imports`, `exports`, or `root`. Used to detect whether an identifier refers to a module-level symbol vs. a local binding. |
| `import(specifier, source)` | Returns the existing local `Id` for this `(specifier, source)` pair, or creates a new synthetic import if it does not yet exist. The new entry goes into both `synthetic` and `rev_imports`. |
| `canonical_id_for(id)` | Returns the canonical (first-seen) `Id` for the symbol name. When multiple identifiers share a name (e.g., after re-export analysis), the canonical ID is always the first one registered. |
| `has_export_symbol(symbol)` | Fast `exports.contains_key(symbol)` check. Used by QwikTransform to determine whether a segment should be named after its export symbol. |
| `export_local_ids()` | Returns all locally-declared identifiers that are exported. Used to determine which root-level bindings need to be preserved as public API. |

* * *

### Pre-Transform Passes

Stages 2–9 run before the core `QwikTransform` (Stage 10). Each subsection documents when the pass runs, what it does, and edge cases an implementor must handle.

#### Stage 2: Strip Exports (`StripExportsVisitor`)

**What:** Removes specified exported symbols by replacing their export declaration with a stub that throws `"Symbol removed by Qwik Optimizer, it can not be called from current platform"`.

**When:** Only runs when `config.strip_exports` is `Some(symbols)`. This is triggered by server-only builds that want to remove client-only exports before shipping the server bundle.

**Mechanism:** `visit_mut_module` iterates module items. For each `export const name = ...` (single declarator) or `export function name`, if `name` is in `filter_symbols`, the entire `ModuleItem` is replaced with:

```js
export const name = () => { throw "Symbol removed by Qwik Optimizer, it can not be called from current platform"; }
```

**Limitation:** Only handles single-declarator `var` exports and `fn` exports. Multi-declarator destructuring exports (`export const { a, b } = ...`) are **not** stripped. `export class` declarations are **not** stripped. Symbols in these forms remain in the output even if listed in `strip_exports`.

**Does not touch:** Imports, non-exported declarations, re-exports from other modules.

Source: `StripExportsVisitor` in `packages/optimizer/core/src/filter_exports.rs`.

#### Stage 3: TypeScript Strip

**What:** SWC's `typescript::strip()` — removes all TypeScript type annotations, type declarations, and type-only imports from the AST.

**When:** `transpile_ts && is_type_script`. Both conditions must be true. `is_type_script` is detected from the file extension during parsing.

**Configuration:** `Default::default()` options with `top_level_mark`.

**Note:** This is SWC standard, not custom code. No Qwik-specific logic is applied.

Source: `swc_ecmascript::transforms::typescript::strip`.

#### Stage 4: JSX Transpile

**What:** SWC's `react::react()` — transforms JSX syntax to `jsx()` / `jsxs()` / `jsxDEV()` calls using the automatic runtime.

**When:** `transpile_jsx && is_jsx`. `is_jsx` is detected from the file extension.

**Configuration:**

```rust
react_options.next = Some(true);
react_options.throw_if_namespace = Some(false);
react_options.runtime = Some(react::Runtime::Automatic);
react_options.import_source = Some("@qwik.dev/core".to_string().into());
```

With `import_source = "@qwik.dev/core"`, the automatic runtime imports from `@qwik.dev/core/jsx-runtime` and `@qwik.dev/core/jsx-dev-runtime`.

This is SWC's standard React-compatible JSX transform. The Qwik-specific JSX rewrite (\`\_jsxSorted\`, \`\_jsxSplit\`) is a \*\*separate pass inside QwikTransform\*\* (Stage 10), not part of this stage. Stage 4 only lowers JSX syntax to function calls. An OXC implementor must not confuse the two — Stage 4 produces \`jsx(Component, { prop: val })\` call shapes; Stage 10 then rewrites those shapes into Qwik's optimized forms.

Source: `swc_ecmascript::transforms::react::react`.

#### Stage 5: Import Rename (`RenameTransform`)

**What:** Renames `@builder.io/*` import source strings to `@qwik.dev/*` equivalents, providing backward compatibility for v1 import paths.

**When:** Always. Unconditional — runs for every file regardless of mode or content.

**Exact substitution rules (checked in this order):**

| Source prefix (v1) | Replacement prefix (v2) | Example |
| --- | --- | --- |
| `@builder.io/qwik-city` | `@qwik.dev/router` | `@builder.io/qwik-city/middleware/node` → `@qwik.dev/router/middleware/node` |
| `@builder.io/qwik-react` | `@qwik.dev/react` | `@builder.io/qwik-react` → `@qwik.dev/react` |
| `@builder.io/qwik` | `@qwik.dev/core` | `@builder.io/qwik/build` → `@qwik.dev/core/build` |

Verbatim Rust implementation:

```rust
// Source: packages/optimizer/core/src/rename_imports.rs lines 8-16
// Checked in order — qwik-city BEFORE qwik (prefix match safety)
if node.src.value.starts_with("@builder.io/qwik-city") {
    node.src.value = ("@qwik.dev/router".to_string() + &node.src.value[21..]).into();
} else if node.src.value.starts_with("@builder.io/qwik-react") {
    node.src.value = ("@qwik.dev/react".to_string() + &node.src.value[22..]).into();
} else if node.src.value.starts_with("@builder.io/qwik") {
    node.src.value = ("@qwik.dev/core".to_string() + &node.src.value[16..]).into();
}
// Suffix offsets: @builder.io/qwik-city = 21 chars, @builder.io/qwik-react = 22 chars,
//                @builder.io/qwik = 16 chars.
```

Order matters. \`@builder.io/qwik-city\` must be checked \*\*before\*\* \`@builder.io/qwik\` because the string \`"qwik-city"\` starts with \`"qwik"\`. A prefix match on \`"@builder.io/qwik"\` alone would match \`"@builder.io/qwik-city"\` first and produce \`"@qwik.dev/core-city"\` — a broken import path. The denylist order (most-specific first) is load-bearing.

**Scope:** Only `ImportDecl.src.value` is rewritten. Export-from declarations (`export { foo } from "@builder.io/qwik"`), dynamic imports, and string literals are **not** affected. This is intentional — only the visitor method `visit_mut_import_decl` is implemented.

Source: `packages/optimizer/core/src/rename_imports.rs` (17 lines total).

#### Stage 6: Resolver

**What:** SWC's `resolver()` — assigns unique `SyntaxContext` hygiene marks to every identifier binding, distinguishing identifiers that spell the same name but were introduced in different scopes.

**When:** Always. Unconditional.

**Configuration:**

```rust
resolver(unresolved_mark, top_level_mark, is_type_script && !transpile_ts)
```

The third argument enables TypeScript-aware resolution **only** when TypeScript has NOT been stripped (i.e., `is_type_script` is true but `transpile_ts` is false). When TypeScript has already been stripped in Stage 3, TypeScript-aware resolution is disabled.

**Side effect:** Creates the `Id = (Atom, SyntaxContext)` pairs that `GlobalCollect` depends on. All subsequent code that uses `Id`\-keyed maps (`imports`, `exports`, `root`, `rev_imports`) depends on this pass having run first. See [The `Id` Type](#the-id-type) above.

Source: `swc_ecmascript::transforms::resolver`.

#### Stage 7: Global Collect

**What:** Runs a single `Visit` pass via `global_collect()` that produces the `GlobalCollect` value passed to all subsequent stages.

See the [GlobalCollect](#globalcollect) section above for full structure, population logic, and invariant documentation. This stage runs a single read-only `Visit` pass over the fully-resolved AST and produces the `GlobalCollect` value. The result is passed by reference into Stage 8 (`transform_props_destructuring`) and by value into Stage 10 (`QwikTransform`).

Source: `global_collect` in `packages/optimizer/core/src/collector.rs` line 56.

#### Stage 8: Props Destructuring (`transform_props_destructuring`)

**What:** Rewrites component-style arrow functions from destructured props parameter form to `_rawProps` parameter form, enabling fine-grained signal-transparent prop forwarding.

**When:** Always — runs for **all** modes including `EmitMode::Lib`. Library `.qwik.mjs` output has this transform pre-applied.

Modes that skip const replacement (\`Lib\`, \`Test\`) do \*\*not\*\* skip props destructuring. This is intentional — library artifacts must have props destructuring applied so consuming apps work correctly without re-running this pass.

**Trigger condition for an individual arrow function:** The arrow must have **exactly one parameter** and a body that is either:

1.  A single expression (no `return` keyword) that is a **call expression**, OR
    
2.  A block statement containing **at least one** `return` **statement**
    

Verbatim Rust trigger detection:

```rust
// Source: visit_mut_arrow_expr in packages/optimizer/core/src/props_destructuring.rs lines 288-306
fn visit_mut_arrow_expr(&mut self, node: &mut ast::ArrowExpr) {
    if node.params.len() == 1 {
        if matches!(&node.body, BlockStmtOrExpr::Expr(Expr::Call(_))) {
            // arrow without return: (props) => someCall(...)
            self.transform_component_props(node);
        } else if matches!(&node.body, BlockStmtOrExpr::BlockStmt(stmts, ..)
            if stmts.iter().any(|s| matches!(s, Stmt::Return(_))))
        {
            // arrow with return: (props) => { ...; return ...; }
            self.transform_component_props(node);
        }
    }
    node.visit_mut_children_with(self);  // recurse into children regardless
}
```

**The rewrite:**

```js
// Before
({ foo, bar }) => { return <div>{foo}{bar}</div>; }

// After
(_rawProps) => {
  const foo = _rawProps.foo;
  const bar = _rawProps.bar;
  return <div>{foo}{bar}</div>;
}
```

Each destructured prop binding is replaced with an inline `_rawProps.propName` property access. The parameter is renamed from the destructuring pattern to `_rawProps`.

**Rest props handling:** If the destructured pattern contains a rest element (`...rest`), the pass adds `const rest = _restProps(_rawProps, ['foo', 'bar'])` at the top of the function body (listing all non-rest props as the exclusion array). If there are no other props, it emits `const rest = _restProps(_rawProps)`. `_restProps` is imported from `core_module` as a synthetic import via `global_collect.import()`.

**Default props:** A prop with a default value (`{ foo = defaultVal }`) is only inlined if `defaultVal` is a constant expression (per `is_const_expr`). If the default value is non-const (e.g., a function call), `transform_pat` returns `None` and the **entire destructuring is skipped** — the arrow function is left unchanged.

**Skip condition for pre-compiled library code:** If the function body's first statement is a `const x = _captures[N]` declaration, the body is skipped entirely. This prevents double-rewriting of `inlinedQrl` function bodies that were already processed in a previous compilation step.

\*\*Pitfall: Props destructuring is not component-only.\*\* The \`PropsDestructuring\` visitor looks up the local \`Id\` for \`component$\` from \`core\_module\` imports for the outer call detection path. However, \`visit\_mut\_arrow\_expr\` independently applies \`transform\_component\_props\` to \*\*any\*\* arrow that matches the shape heuristic (one parameter + call-or-return body), regardless of whether that arrow is a direct argument to \`component$()\`. An OXC implementor must implement both trigger paths: (1) direct \`component$\` argument detection, and (2) the shape-heuristic applied to all matching arrows.

Source: `packages/optimizer/core/src/props_destructuring.rs`.

#### Stage 9: Const Replacement (`ConstReplacerVisitor`)

**What:** Replaces references to `isServer`, `isBrowser`, and `isDev` imported from Qwik core packages with boolean literal values, enabling dead-code elimination of unreachable branches.

**When:** Skipped for `EmitMode::Lib` and `EmitMode::Test`. Runs for all other modes.

The source code uses a \*\*denylist\*\* condition (\`mode != EmitMode::Lib && mode != EmitMode::Test\`), not an allowlist. An allowlist implementation (\`mode == Prod || mode == Dev || mode == Hmr\`) would silently miss any future \`EmitMode\` additions. An OXC reimplementor must use the denylist form to remain correct as the enum evolves.

**The three replaceable constants:**

| Import identifier | Source package | `is_server = true` | `is_server = false` |
| --- | --- | --- | --- |
| `isServer` | `@qwik.dev/core/build` or `@qwik.dev/core` | `true` | `false` |
| `isBrowser` | `@qwik.dev/core/build` or `@qwik.dev/core` | `false` | `true` |
| `isDev` | `@qwik.dev/core/build` or `@qwik.dev/core` | `is_dev` value | `is_dev` value |

Both package sources (`@qwik.dev/core/build` and `@qwik.dev/core`) are equivalent for replacement purposes — the same boolean literal is substituted regardless of which package the identifier was imported from.

**Source lookup:** At construction time, `ConstReplacerVisitor::new` resolves local `Id` values for all six identifiers (three constants × two source packages) from `GlobalCollect.imports` using `get_imported_local(specifier, source)`. Only exact `Id` matches (symbol name + `SyntaxContext`) trigger replacement at runtime. Identifiers that spell the same name but are not imported from a Qwik core package are not affected.

**Why skipped for Lib:** Library mode preserves `isServer`, `isBrowser`, and `isDev` as imports so the consuming application's build performs the replacement with its own `is_server` configuration. The library cannot know whether it will run on server or client.

**Why skipped for Test:** Test mode preserves the constants to allow test code to explicitly control server/browser state through test setup rather than having the optimizer bake in a fixed value.

Source: `packages/optimizer/core/src/const_replace.rs`.

* * *

## Chapter 3: Core QwikTransform

### Overview: What QwikTransform Does

`QwikTransform` is the central fold pass of the optimizer. It implements SWC's `VisitMut` trait and walks the entire module AST in a **single pass**. During this pass it:

1.  Detects `$`\-suffixed marker function calls (e.g., `component$`, `useTask$`, `onClick$`)
    
2.  Extracts their first argument into a `SegmentData` record and decides the output form (extracted file, inline QRL, noop QRL)
    
3.  Computes **scope captures** (`scoped_idents`) — the runtime variables the segment closure reads from its enclosing scope
    
4.  Computes **local idents** (`local_idents`) — the static imports and module-level declarations referenced inside the segment body
    
5.  Generates the appropriate QRL call form at the call site (`qrl(...)`, `inlinedQrl(...)`, or `_noopQrl(...)`)
    
6.  Accumulates `self.segments` — the list of extracted segment records that `parse.rs` later uses to emit the separate segment module files
    

**Key invariant:** This pass runs AFTER all pre-transform passes from Chapter 2. It is Stage 10 in the pipeline. The module has already been resolved (GlobalCollect is populated), imports have been renamed, props destructuring has been rewritten, and const literals have been substituted before `QwikTransform` receives the AST.

**Single pass:** `QwikTransform` processes the entire module in one traversal — there is no second pass for fixup. Fields like `extra_top_items`, `extra_bottom_items`, and `ref_assignments` accumulate side-outputs during folding; `fold_module` drains them at the end of the pass.

### QwikTransform Struct Fields

The following fields are the primary state of the transform. Understanding them is prerequisite to following the segment extraction and hoisting logic.

| Field | Type | Purpose |
| --- | --- | --- |
| `segments` | `Vec<Segment>` | Accumulated extracted segment records; each will produce a segment module file (or an in-module const for Hoist strategy) |
| `stack_ctxt` | `Vec<String>` | Context name stack used to build the segment's `display_name`; pushed/popped as the fold walks into named declarations and marker calls |
| `decl_stack` | `Vec<Vec<IdPlusType>>` | Variable scope stack for capture analysis; each frame is the list of `(Id, IdentType)` pairs for one lexical scope |
| `marker_functions` | `HashMap<Id, Atom>` | Populated at construction from `GlobalCollect.imports`; maps the local `Id` of each `$`\-suffixed import to its specifier name |
| `segment_stack` | `Vec<Atom>` | Tracks active segment symbol names during nested folding; prevents double-extraction when a segment's body contains another `$` call |
| `hoisted_qrls` | `Vec<Vec<(String, VarDeclarator)>>` | Hoisting scope stack; accumulated `.w(...)` const declarations to inject at the top of each function/arrow scope |
| `extra_top_items` | `Vec<ModuleItem>` | Module-scope `const q_name = qrl(...)` declarations added during hoisting; drained to the top of the module by `fold_module` |
| `extra_bottom_items` | `Vec<ModuleItem>` | Auto-export statements for `local_idents` that were not already exported; drained to the bottom of the module by `fold_module` |
| `ref_assignments` | `Vec<ExprStmt>` | `.s()` assignment statements for hoisted `inlinedQrl` bindings; emitted at module scope after the `_noopQrl` const |
| `iteration_var_stack` | `Vec<Vec<Id>>` | Tracks loop iteration variables (from `for`, `.map()`, etc.); used by `compute_hoist_target_depth` to determine the shallowest safe hoist target |
| `component_depths` | `Vec<usize>` | Stack of hoisting scope depths for `component$` boundaries; used to bound `.w()` hoist targets to within the component |
| `segment_names` | `HashMap<String, u32>` | Per-file collision counter for display names; if two segments in the same file produce the same display name, the second gets `_1`, `_2`, etc. |

Source: `QwikTransform` struct in `packages/optimizer/core/src/transform.rs`.

### Marker Function Detection

#### How `marker_functions` Is Populated (XFRM-01)

`QwikTransform::new()` builds the `marker_functions` map by scanning `GlobalCollect.imports` and `GlobalCollect.export_local_ids()` before any folding begins:

```rust
// Source: QwikTransform::new() in transform.rs lines 191-202
for (id, import) in options.global_collect.imports.iter() {
    if import.kind == ImportKind::Named && import.specifier.ends_with(QRL_SUFFIX) {
        marker_functions.insert(id.clone(), import.specifier.clone());
    }
}
for id in options.global_collect.export_local_ids() {
    if id.0.ends_with(QRL_SUFFIX) {
        marker_functions.insert(id.clone(), id.0.clone());
    }
}
```

**Rule:** Any named import whose `specifier` ends with `$` is a marker function. Any locally-exported identifier ending with `$` is also a marker function. The map value is the specifier name (e.g., `"component$"`, `"useTask$"`, `"onClick$"`).

**Special-case functions handled directly (NOT via** `marker_functions`**):**

| Identifier | Import name | Handler function |
| --- | --- | --- |
| `qsegment_fn` | `$` (raw segment, `QSEGMENT` atom) | `handle_qsegment` |
| `sync_qrl_fn` | `sync$` | `handle_sync_qrl` |
| `inlined_qrl_fn` | `inlinedQrl` | `handle_inlined_qsegment` |
| `jsx_functions` | `jsx`, `jsxs`, `jsxDEV`, jsx-runtime | `handle_jsx` |
| `fn_signal_fn` | `_fnSignal` | `hoist_fn_signal_call` |

These five are resolved at construction time (as separate `Option<Id>` fields on `QwikTransform`) and checked before the `marker_functions` lookup in `fold_call_expr`.

#### `fold_call_expr` Decision Tree

`fold_call_expr` receives every `CallExpr` node in the module. The checks are ordered by priority:

```plaintext
fold_call_expr(call_expr):
  Is callee a simple Ident?
    No  → fold children unchanged; return
    Yes → check identity in priority order:
      1. sync_qrl_fn?        → handle_sync_qrl → return
      2. qsegment_fn?        → handle_qsegment → hoist_qrl_to_module_scope
                                → pending_expr_replacement → return
      3. jsx_functions?      → handle_jsx → return
      4. inlined_qrl_fn?     → handle_inlined_qsegment → hoist_qrl_to_module_scope
                                → pending_expr_replacement → return
      5. fn_signal_fn?       → fold children + hoist_fn_signal_call → return
      6. marker_functions?   → _create_synthetic_qsegment(first arg)
                                → hoist_qrl_to_module_scope
                                → callee rewritten: $-suffix → Qrl-suffix
      7. any other ident     → push ident name to stack_ctxt
                                → fold children
                                → pop stack_ctxt
```

**The** `convert_qrl_word` **rewrite (step 6):** When a marker function is matched, its callee identifier is renamed from the `$`\-suffix form to the `Qrl`\-suffix equivalent:

| User code | Output |
| --- | --- |
| `component$(fn)` | `componentQrl(extractedQrl)` |
| `useTask$(fn)` | `useTaskQrl(inlinedQrl)` |
| `useOn$('click', fn)` | `useOnQrl('click', qrl(...))` |
| `onClick$(fn)` | `onClickQrl(qrl(...))` |

The original `$`\-suffix form never appears in optimizer output. The Qwik runtime functions all accept the `Qrl`\-suffix form.

### Segment Extraction Decision Tree

#### `_create_synthetic_qsegment` (XFRM-01)

This is the core function where all three sub-systems converge: marker detection routes here; this function decides the output form; scope capture analysis and symbol naming run inside it.

**Step 0: Const initializer inlining** *(non-Lib, non-Inline/Hoist only)*

Before any other processing, if the first argument to the marker function is a simple `Ident`, and that ident is not exported, and `self.const_initializers` has a stored initializer for it, substitute the stored initializer expression in place of the ident.

```plaintext
// Rationale: fixes the pattern:
//   const style = '...css...';
//   useStyles$(style);
// Without this substitution, the segment would receive `style` as an
// identifier — but `style` would be undefined inside the segment module
// because it is not exported. The initializer inline makes the segment
// self-contained.
```

> **Note:** This substitution only runs when mode is NOT `Lib` and strategy is NOT `Inline` or `Hoist`. In Lib mode, the Lib early-return path handles the expression directly. In Inline/Hoist strategies, the segment stays in-module so the variable reference is still valid.

**Step 1: Import QRL name detection**

`get_import_qrl_name(first_arg)` inspects the first argument:

*   If `first_arg` is an `Ident` that points to an import in `GlobalCollect` → extract `display_name` and `hash_seed` from the import record
    
*   If `first_arg` is a `Member` expression from a namespace import → extract the member name and namespace hash seed
    

These values give stable hash inputs to cross-file QRL references (e.g., `component$(imported.handler)` gets the same hash regardless of which file does the wrapping).

**Step 2: EmitMode::Lib early-return**

If `self.options.mode == EmitMode::Lib` → take the Lib path (see [EmitMode::Lib Contract](#emitmode-lib-contract) below). The Lib path returns an `inlinedQrl(...)` call and does NOT push to `self.segments`.

**Step 3: Non-Lib path — symbol naming and AST folding**

```plaintext
register_context_name(custom_symbol, display_name_override, hash_override)
  → (symbol_name, display_name, hash, segment_hash)

Collect descendent_idents via IdentCollector (all Id refs in first_arg)

Partition decl_stack entries:
  decl_collect ← entries with IdentType::Var(_)   // variables
  invalid_decl  ← entries with IdentType::Fn | IdentType::Class  // functions/classes

segment_stack.push(symbol_name)
  fold first_arg recursively (nested $ calls produce their own segments)
segment_stack.pop()

get_local_idents(&folded)  → local_idents    // module-level refs → become imports
get_function_params(&folded) → param_idents  // fn params (excluded from captures)
compute_scoped_idents(descendent_idents, decl_collect) → (scoped_idents, is_const)
scoped_idents.retain(|id| !param_idents.contains(id))  // remove params
```

**Step 4:** `can_capture` **check**

`can_capture_scope(first_arg)` returns `true` only if `first_arg` is a function expression (`Expr::Fn`) or arrow expression (`Expr::Arrow`).

```plaintext
If !can_capture && !scoped_idents.is_empty():
  → emit CanNotCapture diagnostic (error code C03)
  → set scoped_idents = []
```

Non-function expressions (e.g., an object literal) cannot be turned into a lazily-evaluated closure — if they reference outer variables, that is an error.

**Step 5:** `should_emit` **check — the three output branches**

```plaintext
should_emit_segment(ctx_name, ctx_kind) → bool

If !should_emit:
  → create_noop_qrl(symbol_name, scoped_idents)   // stripped segment

If should_emit && is_inline():
  → create_inline_qrl(segment_data, folded, symbol_name)  // inlinedQrl(...)

If should_emit && !is_inline():
  → create_segment(segment_data, folded)           // qrl(() => import('./chunk'), ...)
    → pushes to self.segments
```

`is_inline()` gate:

```rust
// Source: QwikTransform::is_inline lines 313-318
const fn is_inline(&self) -> bool {
    matches!(
        self.options.entry_strategy,
        EntryStrategy::Inline | EntryStrategy::Hoist
    )
}
```

\`EntryStrategy::Single\` is NOT in the \`is\_inline()\` gate, despite producing the same \`'entry\_segments'\` manifest key as \`Inline\` and \`Hoist\`. Only \`Inline\` and \`Hoist\` produce \`inlinedQrl()\` output. \`Single\` produces extracted \`qrl()\` calls pointing to a single shared entry chunk. An OXC implementor must not conflate the three strategies.

**Step 6:** `local_idents` **validation** *(only when* `should_emit` *is true)*

For each identifier in `local_idents` that is not already exported from the source module:

```plaintext
ensure_export(id) → adds a synthetic export statement to extra_bottom_items
if id is in invalid_decl (Fn or Class):
  → emit FunctionReference diagnostic (error code C02)
```

A function or class declared in the module scope but not exported cannot be imported by the segment file. The C02 diagnostic tells the user to move the function into the segment or export it explicitly.

#### `should_emit_segment` Conditions

A segment is suppressed (noop QRL emitted) when ANY of the following is true:

| Condition | When set | Effect |
| --- | --- | --- |
| `strip_ctx_name` is set AND `ctx_name.starts_with(strip_ctx_name_item)` | Server builds that want to strip specific context (e.g., server-only segments) | Emits `_noopQrl` for matching segments |
| `strip_event_handlers` is true AND `ctx_kind == SegmentKind::EventHandler` | Server builds that do not need client event code | Emits `_noopQrl` for all event handler segments |

Both conditions may apply simultaneously; either one is sufficient to suppress emission.

### EmitMode::Lib Contract

#### The Lib Early-Return Path (XFRM-06)

When `mode == EmitMode::Lib`, `_create_synthetic_qsegment` takes a completely separate path from the non-Lib path. This is not a flag variation — it is a distinct output contract.

**The 10-step Lib path:**

```plaintext
1. register_context_name(custom_symbol, display_name_override, hash_override)
     → (symbol_name, display_name, hash, _)

2. Collect descendent_idents via IdentCollector

3. Build decl_collect from decl_stack (Var entries only — same as non-Lib Step 3)

4. segment_stack.push(symbol_name)
     fold first_arg recursively
   segment_stack.pop()

5. param_idents = get_function_params(&folded)

6. compute_scoped_idents(descendent_idents, decl_collect) → (scoped_idents, _)

7. scoped_idents.retain(|id| !param_idents.contains(id))

8. If !can_capture && !scoped_idents.is_empty():
     emit CanNotCapture diagnostic (C03)
     set scoped_idents = []

9. If scoped_idents is non-empty:
     ensure_core_import(_CAPTURES) → new_local
     transform_function_expr(folded, &new_local, &scoped_idents)
     // Injects: const varN = _captures[index] at top of function body
     // for each captured variable in scoped_idents

10. Build SegmentData with local_idents = []   // always empty in Lib mode
    Return create_inline_qrl(segment_data, folded, symbol_name, span)
```

#### Key Differences: Lib vs Non-Lib

| Behavior | Non-Lib path | Lib path |
| --- | --- | --- |
| `local_idents` | Computed via `get_local_idents` → become segment file imports | Always `vec![]` — no import generation |
| `self.segments` push | Performed for extracted QRLs (`create_segment`) | **Never** — no separate segment files |
| `_captures` destructuring | Not injected (runtime capture array passed through QRL call mechanism) | Injected into function body: `const varN = _captures[index]` |
| QRL call form returned | `qrl(...)`, `inlinedQrl(...)`, or `_noopQrl(...)` depending on `should_emit` and `is_inline()` | Always `inlinedQrl(...)` — no `should_emit` check |
| Module-scope hoisting | `hoist_qrl_to_module_scope` generates `const q_name = ...` | **Blocked** — Lib guard returns immediately |

#### No Module-Scope Hoisting in Lib

```rust
// Source: transform.rs lines 1459-1463
fn hoist_qrl_to_module_scope(&mut self, call_expr: ast::CallExpr) -> ast::Expr {
    if matches!(self.options.mode, EmitMode::Lib) {
        return ast::Expr::Call(call_expr);  // Return unchanged
    }
    // ...non-Lib hoisting logic...
```

The `inlinedQrl(...)` call is returned directly at its use site. No `const q_name = ...` is generated at module scope. No `.s()` ref assignments are added. The `inlinedQrl` in the library output is fully self-contained.

#### `_captures` Destructuring in Lib Output

When a Lib segment has captures, `transform_function_expr` (from `code_move.rs`) injects one `const` statement per captured variable at the top of the function body:

```js
// Input:
export const Counter = component$(() => {
  const count = useSignal(0);
  return useTask$(() => {           // ← count is captured
    console.log(count.value);
  });
});

// Lib output (for the inner useTask$ segment):
inlinedQrl(
  (_captures) => {
    const count = _captures[0];    // one per captured var, in scoped_idents order
    console.log(count.value);
  },
  'Counter_useTask_AbCdEfGhIjK',
  [count]                          // runtime capture array
)
```

Each captured variable `varN` becomes `const varN = _captures[index]` — by numeric index, in the order produced by `compute_scoped_idents` (which sorts the output `Vec<Id>` for determinism).

> **Note:** The `_captures` parameter is injected into the function signature. It is NOT a user-visible identifier — it is the parameter name added by `transform_function_expr`. The destructuring statements at the top of the body use positional indexing (`_captures[0]`, `_captures[1]`, ...) rather than named keys.

* * *

### Scope Capture Analysis (XFRM-02)

\*\*Highest-risk area for OXC reimplementation.\*\* The terms \`scoped\_idents\` and \`local\_idents\` are NEVER interchangeable. They describe entirely different identifier sets, computed by different algorithms, flowing into completely different output mechanisms. Conflating them is the most common implementation error in alternate optimizer ports.

#### The Two Terms: Precise Definitions

| Term | What it Is | How Computed | Output Mechanism |
| --- | --- | --- | --- |
| `scoped_idents` | Variables from the **enclosing scope** that the segment closure reads at runtime | `compute_scoped_idents`: intersection of `descendent_idents` with `decl_collect` (the `Var` entries from `decl_stack`) | Emitted as the capture array in `qrl(..., [vars])` or `inlinedQrl(..., ..., [vars])`; injected as `const varN = _captures[index]` inside the segment body |
| `local_idents` | **Imports and module-level declarations** referenced inside the segment body | `get_local_idents`: `IdentCollector` over the **folded** expr, filtered by `locally_declared` — returns idents that are globals (present in `global_collect`) | Become `import` statements at the top of the emitted segment file; if not yet exported from parent module, `ensure_export` adds a synthetic `_auto_name` export |

The distinction matters at every step:

*   `scoped_idents` → runtime binding (passed as array, destructured inside body)
    
*   `local_idents` → static binding (resolved at import time, not passed at runtime)
    

Source: `compute_scoped_idents` in `packages/optimizer/core/src/transform.rs` lines 4894–4908; `get_local_idents` in `transform.rs` lines 1077–1105

#### IdentType Enum

```rust
// Source: transform.rs lines 91-95
pub enum IdentType {
    Var(bool),  // bool = is_const_and_static
    Fn,
    Class,
}
```

The four variants:

*   `Var(true)` — a `const` variable whose initializer is also statically constant (verified by `collect_static_identifiers`). These variables are safe to inline — they will not change at runtime.
    
*   `Var(false)` — a `let` or `var` variable, OR a `const` with a non-static initializer (e.g., `const x = someSignal.value`). These require runtime capture.
    
*   `Fn` — a function declaration. The function name is registered in `decl_stack` as an `Fn` entry.
    
*   `Class` — a class declaration.
    

**The** `invalid_decl` **partition:** In `_create_synthetic_qsegment`, `decl_stack` is split into two sets:

*   `decl_collect` — the `Var` entries (used by `compute_scoped_idents`)
    
*   `invalid_decl` — the `Fn` and `Class` entries
    

References to functions or classes inside a segment trigger the **C02** `FunctionReference` **diagnostic** (emitted in Step 6 of the decision tree) because function and class declarations cannot be serialized for lazy loading.

#### compute\_scoped\_idents

```rust
// Source: transform.rs lines 4894-4908
fn compute_scoped_idents(all_idents: &[Id], all_decl: &[IdPlusType]) -> (Vec<Id>, bool) {
    let mut set: HashSet<Id> = HashSet::new();
    let mut is_const = true;
    for ident in all_idents {
        if let Some(item) = all_decl.iter().find(|item| item.0 == *ident) {
            set.insert(ident.clone());
            if !matches!(item.1, IdentType::Var(true)) {
                is_const = false;
            }
        }
    }
    let mut output: Vec<Id> = set.into_iter().collect();
    output.sort();
    (output, is_const)
}
```

**What it does:** Intersects `all_idents` (every identifier used in the expression body, collected before folding via `IdentCollector`) with `all_decl` (the `Var` entries from `decl_stack` that are in scope at the call site). Each ident that appears in both sets must be captured.

**The** `is_const` **boolean:** `true` only if ALL captured variables are `Var(true)`. This flag controls `_fnSignal` optimization eligibility — if `is_const` is false, the capture array contains mutable bindings and the `_fnSignal` path is unavailable.

**Post-computation cleanup:** After `compute_scoped_idents` returns, function parameters are removed:

```rust
scoped_idents.retain(|id| !param_idents.contains(id));
```

A function's own parameters are in scope during traversal but do not need to be captured — they are provided at the call site, not from the enclosing scope.

**Output ordering:** The output `Vec<Id>` is sorted (line `output.sort()`), making the capture array order deterministic across builds.

Source: `compute_scoped_idents` in `transform.rs` lines 4894–4908

#### get\_local\_idents

```rust
// Source: transform.rs lines 1077-1105
fn get_local_idents(&self, expr: &ast::Expr) -> Vec<Id> {
    // 1. Run IdentCollector over expr to get all referenced identifiers
    // 2. Filter out identifiers declared INSIDE the expression body
    //    (locally-declared vars would shadow outer names — exclude them)
    // 3. Add `h` and `Fragment` identifiers if the expression contains JSX
    //    (these are implicitly required by JSX desugaring)
    // Returns: identifiers that reference things OUTSIDE the expression body
}
```

**Critical difference from** `scoped_idents`**:** `get_local_idents` is called on the **folded** expression — after the recursive `QwikTransform` has already processed all nested `$` calls inside the body. It finds identifiers that reference the global scope (imports and module-level declarations present in `global_collect`).

These identifiers are not captured at runtime; they are statically imported. The emitted segment file will contain `import { name } from './parent'` statements for each one. If the identifier is not yet exported from the parent module, `ensure_export` adds a synthetic named export (`export { name as _auto_name }`) to `extra_bottom_items`.

Source: `get_local_idents` in `transform.rs` lines 1077–1105

#### decl\_stack Management

The `decl_stack: Vec<Vec<IdPlusType>>` is a stack of scopes. Each frame is a `Vec<(Id, IdentType)>`. The stack tracks which identifiers are declared in each lexical scope.

**Scope mutation points:**

| Visitor method | Action |
| --- | --- |
| `fold_var_decl` | Pushes `Var(is_const && is_static)` for each declared variable into the **current** scope (`decl_stack.last_mut()`) |
| `fold_fn_decl` | Pushes `Fn` for the function name into the **current** scope |
| `fold_function` | Pushes a **new** scope, adds all function params as `Var(false)`, folds children, pops scope |
| `fold_arrow_expr` | Pushes a **new** scope, adds all arrow params as `Var(false)`, folds children, pops scope |

`collect_static_identifiers`**:** Determines whether a `const` declarator's initializer is purely static. If `const x = 5`, `x` is `Var(true)`. If `const x = someSignal.value`, the `.value` access makes it non-static — `x` is `Var(false)`.

**Scope accumulation for** `compute_scoped_idents`**:** Before calling `compute_scoped_idents`, all `decl_stack` frames are flattened into `decl_collect` (keeping only `Var` entries). This means the intersection is against ALL variables declared in any enclosing scope at the call site, not just the innermost frame.

Source: `fold_var_decl`, `fold_fn_decl`, `fold_function`, `fold_arrow_expr` in `transform.rs`

* * *

### Symbol Naming Algorithm (XFRM-03)

The symbol name is the stable string ID that uniquely identifies a segment across builds. It appears as the `symbolName` argument in `qrl()`, `inlinedQrl()`, and `_noopQrl()` calls, and as the export name in the emitted segment file. **OXC must produce byte-identical symbol names** — any deviation breaks cross-build caching, manifest matching, and resumability.

The algorithm is implemented in `register_context_name` and is a 6-step pipeline.

Source: `register_context_name` in `transform.rs` lines 353–422; `escape_sym` lines 4612–4635; `base64` lines 4725–4729; `get_canonical_filename` lines 4910–4912; `parse_symbol_name` lines 4915–4928

#### Step 0: Custom Symbol Bypass

If a `custom_symbol` is provided (from `qsegment$` second-argument overrides), it is returned directly as the symbol name with no hash computation:

```plaintext
IF custom_symbol provided:
  return (custom_symbol, custom_symbol, custom_symbol, 0)
```

The hash pipeline is entirely skipped. Custom symbols must be globally unique — the caller is responsible for uniqueness guarantees.

#### Step 1: Build display\_name

```plaintext
display_name =
  IF display_name_override provided:
    use display_name_override (supplied for import-QRL names to give stable cross-file hashes)
  ELSE:
    join stack_ctxt entries with '_' separator
    IF stack_ctxt is empty: prefix result with 's_'

apply escape_sym(display_name):
  replace all non-alphanumeric characters with '_'
  squash consecutive underscores into one '_'
  trim leading underscores

IF first character of result is a digit: prefix with '_'
```

`escape_sym` **behavior:**

```rust
// Source: transform.rs lines 4612-4635
// "my-component.handler" → "my_component_handler"
// "---foo"               → "foo"
// "__bar"                → "bar"
// "123click"             → "_123click"
```

The `stack_ctxt` is the context stack built by `fold_call_expr` as it descends through the AST. For a `component$` at the top of a file, `stack_ctxt` might be `["Counter"]`. For an inner `useTask$`, it might be `["Counter", "useTask"]`. This is what produces human-readable symbol names like `Counter_useTask_AbCdEfGhIjK`.

#### Step 2: Collision Counter

```plaintext
segment_names: HashMap<String, u32>  // persists for the entire module transform

look up display_name in segment_names:
  NOT found: insert with value 0; index = 0 (no suffix appended)
  FOUND:     increment stored value; index = new value

IF index > 0: append '_N' to display_name
  e.g. first 'onClick' → 'onClick' (index 0, no suffix)
       second 'onClick' → 'onClick_1' (index 1)
       third 'onClick'  → 'onClick_2' (index 2)
```

The collision counter ensures that two segments with the same context stack (e.g., two `onClick$` handlers in the same component) produce distinct symbol names. The suffix is appended to `display_name` **before** hash computation (Step 3), so different occurrences produce different hashes.

#### Step 3: Hash Computation

```plaintext
create DefaultHasher (Rust's std::collections::hash_map::DefaultHasher — SipHash 1-3)

IF hash_override provided:
  write hash_override bytes only

ELSE:
  IF options.scope is Some(scope): write scope bytes
  write rel_path bytes (forward-slash normalized via to_slash_lossy)
  write display_name bytes (AFTER collision suffix from Step 2)

hash: u64 = hasher.finish()
```

**Implementation note for OXC:** `DefaultHasher` uses SipHash 1-3 with a fixed seed of `(0, 0)`. The byte order of multi-byte fields follows Rust's `Hasher::write` semantics (writes bytes directly, not values). The path normalization converts Windows backslashes to forward slashes before hashing.

#### Step 4: Base64 Encoding

```rust
// Source: transform.rs lines 4725-4729
fn base64(nu: u64) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(nu.to_le_bytes())
        .replace(['-', '_'], "0")
}
```

*   `nu.to_le_bytes()` — the u64 is serialized as **8 bytes in little-endian order** before base64 encoding
    
*   `URL_SAFE_NO_PAD` — uses the URL-safe base64 alphabet (no `+`/`/`; uses `-`/`_`) without padding characters
    
*   `.replace(['-', '_'], "0")` — both `-` and `_` characters in the base64 output are replaced with `0`, ensuring the result is a valid JavaScript identifier with no URL-special characters
    
*   Result: always **11 characters** (8 bytes × 8 bits / 6 bits per base64 char = 10.67, rounded up to 11)
    

OXC must use \*\*identical hash parameters\*\*: Rust \`DefaultHasher\` (SipHash 1-3, seed \`(0,0)\`), \*\*little-endian\*\* byte serialization of the u64, \`URL\_SAFE\_NO\_PAD\` base64 variant, and the same \`replace(\['-', '\_'\], "0")\` post-processing. Any single deviation produces different 11-character hashes and breaks cross-implementation symbol name compatibility.

#### Step 5: Format symbol\_name by Mode

| Mode | `symbol_name` format | Example |
| --- | --- | --- |
| `Dev`, `Test`, `Hmr`, `Lib` | `{display_name}_{hash64}` | `onClick_AbCdEfGhIjK` |
| `Prod` | `s_{hash64}` | `s_AbCdEfGhIjK` |

In `Prod` mode, the display name is stripped from the symbol for minification. The symbol is only 13 characters regardless of how long the context stack is. In all other modes, the full display name is preserved for debuggability.

#### Step 6: Format Final display\_name

```plaintext
display_name = "{file_name}_{display_name}"
```

Where `file_name` is `options.path_data.file_name` — the source file's base name without extension (e.g., `counter` for `counter.tsx`). This is the display name that appears in Dev/Hmr source maps and diagnostics.

**Return value:** `(symbol_name, display_name, hash64_atom, segment_hash_u64)`

#### get\_canonical\_filename

```rust
// Source: transform.rs lines 4910-4912
fn get_canonical_filename(display_name: &Atom, symbol_name: &Atom) -> Atom {
    let hash = symbol_name.split('_').next_back().unwrap();
    Atom::from(format!("{}_{}", display_name, hash))
}
```

The canonical filename is `{display_name}_{hash}`. The hash segment is extracted from the **symbol name's** last `_`\-delimited token. This works for both modes:

*   **Dev mode**: `symbol_name` = `onClick_AbCdEfGhIjK` → `hash` = `AbCdEfGhIjK` → canonical = `counter_onClick_AbCdEfGhIjK`
    
*   **Prod mode**: `symbol_name` = `s_AbCdEfGhIjK` → `hash` = `AbCdEfGhIjK` → canonical = `counter_onClick_AbCdEfGhIjK`
    

Both modes produce the same canonical filename, which is used as the segment file name on disk.

#### parse\_symbol\_name

```rust
// Source: transform.rs lines 4915-4928
// Used when processing existing inlinedQrl() calls from library code
// (handle_inlined_qsegment path)
//
// Input: a symbol_name string (e.g., "onClick_AbCdEfGhIjK" or "s_AbCdEfGhIjK")
// Splits on last '_' to extract the hash segment
// In Dev/Test/Hmr/Lib mode: symbol_name stays as-is
// In Prod mode: symbol_name is reformatted as "s_{hash}"
// display_name is always "{file_name}_{original_display_name}"
```

This function is called by `handle_inlined_qsegment` when re-processing `inlinedQrl()` calls that appear in imported library code. The library was compiled with its own symbol names; `parse_symbol_name` extracts the hash and reformats the symbol according to the current build's mode.

Source: `register_context_name` in `packages/optimizer/core/src/transform.rs` lines 353–422; `escape_sym` lines 4612–4635; `base64` lines 4725–4729; `get_canonical_filename` lines 4910–4912; `parse_symbol_name` lines 4915–4928

### QRL Call Forms

After `_create_synthetic_qsegment` determines which branch to take, one of four QRL call form generators is invoked. The choice is determined by the combination of `should_emit`, `is_inline()`, and `EmitMode`.

| Form | Rust Function | JS Output | When Used |
| --- | --- | --- | --- |
| Extracted QRL | `create_segment` + `create_qrl` | `qrl(() => import('./chunk'), 'symbolName')` | Non-inline strategy, `should_emit=true`, Prod/Test mode |
| Extracted QRL (Dev) | `create_segment` + `create_qrl` | `qrlDEV(() => import('./chunk'), 'symbolName', {file, lo, hi, displayName})` | Dev or Hmr mode, non-inline, `should_emit=true` |
| Inline QRL | `create_inline_qrl` | `inlinedQrl(fn, 'symbolName')` | Inline/Hoist strategy OR Lib mode, `should_emit=true` |
| Inline QRL (Dev) | `create_inline_qrl` | `inlinedQrlDEV(fn, 'symbolName', {file, lo, hi, displayName})` | Dev/Hmr + Inline/Hoist strategy |
| Noop QRL | `create_noop_qrl` | `_noopQrl('symbolName')` | `should_emit=false` (stripped segment) |
| Noop QRL (Dev) | `create_noop_qrl` | `_noopQrlDEV('symbolName', {file, lo, hi, displayName})` | Dev/Hmr + `should_emit=false` |

The Dev/Hmr variants append a metadata object `{file, lo, hi, displayName}` as a trailing argument for source map enrichment and diagnostics. Prod/Test variants omit this object entirely.

#### Capture Array Emission

When `scoped_idents` is non-empty, `emit_captures` appends a capture array literal as the last argument to the QRL call. All three QRL forms accept this trailing array:

```js
// Extracted QRL with captures
qrl(() => import('./chunk'), 'symbolName', [capturedVar1, capturedVar2])

// Inline QRL with captures
inlinedQrl(fn, 'symbolName', [capturedVar1, capturedVar2])

// Noop QRL with captures
_noopQrl('symbolName', [capturedVar1, capturedVar2])
```

Two capture modes exist:

*   `Captures::Auto(ids)` — emits an inline array of the captured `Id`s as identifier expressions. This is the standard path for all newly-compiled segments.
    
*   `Captures::Explicit(arr)` — emits the literal `ArrayLit` AST node as-is. Used when re-processing pre-existing `inlinedQrl()` calls from library code (`handle_inlined_qsegment` path). The explicit form supports non-ident expressions in the capture array (e.g., `someObj.value`), which is the fix from PR #8493.
    

#### create\_segment: Segment Record Push

`create_segment` is the output mechanism for non-inline extracted segments. It:

1.  Computes `canonical_filename` from `display_name` + symbol hash (via `get_canonical_filename`)
    
2.  Calls `entry_policy.get_entry_for_sym()` to obtain the chunk entry name
    
3.  Builds the import path: `./{canonical_filename}` (with extension appended if `explicit_extensions` is set)
    
4.  Calls `create_qrl()` to produce the `qrl(() => import('...'), 'symbolName')` call expression
    
5.  Pushes `Segment { entry, span, canonical_filename, name, data, expr, hash, param_names }` to `self.segments`
    
6.  Returns the `qrl()` `CallExpr`
    

The push to `self.segments` is a side effect. After the entire module fold completes, `parse.rs` consumes `self.segments` to generate the separate segment module files on disk.

Source: `create_segment` in `packages/optimizer/core/src/transform.rs` lines 1110–1144; `create_qrl` lines ~1870–1943

#### \_noopQrl: Current State and Planned Change

The `create_noop_qrl` function is called when `should_emit_segment` returns `false` — for segments that are deliberately stripped from the current build (e.g., server-side event handlers stripped during client builds).

**Current behavior:** `create_noop_qrl` does NOT push any entry to `self.segments`. Noop QRLs produce no `SegmentAnalysis` entry in the manifest. The symbol name is still computed and passed as a string argument, but no segment file is generated.

The exact location in the source where this would change is flagged with:

```rust
// TODO export segment data for the noop qrl
// Source: transform.rs line 2999
```

**Planned change (routeloader-split):** In the planned `routeloader-split` work, `create_noop_qrl` will push a `Segment { entry: None, ... }` to `self.segments`, producing `SegmentAnalysis { entry: None }` entries for server-side manifest generation. This allows the server build to know which symbols exist even though their segments are stripped.

The noop SegmentAnalysis emission is a known planned change tracked in the \`routeloader-split\` work. The TODO comment at transform.rs line 2999 is the precise insertion point. Until this change lands, OXC implementors should match current behavior (no SegmentAnalysis for stripped segments) and add a flag for the planned change.

Source: `create_noop_qrl` in `packages/optimizer/core/src/transform.rs` lines 3000–3027

#### Inline QRL with Hoist Strategy

When `entry_strategy == Hoist`, `create_inline_qrl` takes a specialized path:

1.  Creates a `private_ident!` — a fresh, generated unique symbol identifier — for the function body
    
2.  Pushes a `Segment { entry: None, ... }` to `self.segments` (NOT a separate file — `entry: None` signals in-module placement)
    
3.  The `Segment`'s `expr` field holds the function body; `fold_module`'s Hoist drain code emits it as a `const` declaration at module scope
    
4.  Returns `inlinedQrl(private_ident, 'symbolName')`
    

The `qrl_id` field on these segments carries the hoisted const's `Id`, which is used for `.s()` call emission during module-scope hoisting (see Two-Level QRL Hoisting below).

The key distinction: Hoist strategy segments DO appear in `self.segments` (unlike Lib mode), but their `entry` is `None` — they become in-module `const` declarations rather than separate output files.

Source: `create_inline_qrl` in `packages/optimizer/core/src/transform.rs` lines 1945–2011

### Two-Level QRL Hoisting

QRL hoisting serves two distinct purposes: (1) **deduplication** — module-scope `const` declarations ensure each unique QRL base object is created once, not once per usage; and (2) **loop optimization** — `.w()` (with-captures) expressions are hoisted to the outermost scope where all captures are still available, avoiding per-iteration recreation inside loops.

These two levels are implemented by two separate functions: `hoist_qrl_to_module_scope` (Level 1) and `hoist_qrl_if_needed` (Level 2).

#### Level 1: Module-Scope Const Hoisting

`hoist_qrl_to_module_scope` is called immediately after `create_segment` or `create_inline_qrl` returns a QRL call expression. It hoists the QRL base to module scope and replaces the call site with a reference to the hoisted const.

**EmitMode::Lib guard — early return:**

```rust
// Source: hoist_qrl_to_module_scope in transform.rs lines 1459-1463
fn hoist_qrl_to_module_scope(&mut self, call_expr: ast::CallExpr) -> ast::Expr {
    if matches!(self.options.mode, EmitMode::Lib) {
        return ast::Expr::Call(call_expr);  // Return unchanged
    }
```

In `Lib` mode, no module-scope hoisting occurs. The `inlinedQrl(...)` call is returned as-is at its use site. This is part of the Lib output contract: no `const q_name = ...` declarations, no `.s()` ref\_assignments (see XFRM-06).

**For extracted** `qrl()` **calls (non-Lib, non-inline strategy):**

1.  Extract capture array from args (if present)
    
2.  Build hoisted const name: `q_{symbol_name}` (e.g., `q_onClick_AbCdEfGhIjK`)
    
3.  If `extra_top_items` does not already contain this Id: add `const q_name = qrl(() => import('./chunk'), 'name')` to `extra_top_items`
    
4.  Replace call site with `q_name` ident (no captures) OR `q_name.w([captures])` (with captures)
    

```js
// Before hoisting:
componentQrl(qrl(() => import('./counter_Component_abc'), 'Component_abc'))

// After Level 1 hoisting:
const q_Component_abc = qrl(() => import('./counter_Component_abc'), 'Component_abc');
componentQrl(q_Component_abc)
```

**For** `inlinedQrl()` **calls (Inline/Hoist strategy):**

1.  Extract capture array and fn\_body (first arg) from the `inlinedQrl(fn, 'name')` call
    
2.  Build a `_noopQrl('symbolName')` call (or `_noopQrlDEV` in Dev/Hmr mode)
    
3.  Hoist `const q_name = _noopQrl('symbolName')` to `extra_top_items`
    
4.  Add `q_name.s(fn_body)` to `ref_assignments` — emitted at module scope after the `_noopQrl` const declaration
    
5.  Replace call site with `q_name` ident (no captures) OR `q_name.w([captures])` (with captures)
    

**Non-global ident special case:** If `fn_body` is a local variable not present in `global_collect` (i.e., it is not accessible at module scope), the `.s()` call cannot be added to `ref_assignments` (because `ref_assignments` is emitted at module top-level). Instead, emit a comma expression at the call site:

```js
(q_name.s(localVar), q_name)
```

This pattern ensures the `.s()` setter is called at the moment when `localVar` is in scope.

Source: `hoist_qrl_to_module_scope` in `packages/optimizer/core/src/transform.rs` lines 1459–1668

#### Level 2: Loop-Context .w() Hoisting

Inside a loop (e.g., `.map()` callbacks), the Level 1 module-scope const `q_name` has no captures. But `q_name.w([item])` would be recreated on every loop iteration — one `.w()` allocation per element. Level 2 hoisting avoids this by moving the `.w(captures)` call to the outermost scope where all captured variables are still in scope.

`hoist_qrl_if_needed` is called when ALL of:

*   Inside a loop: `iteration_var_stack` is non-empty
    
*   Not a component root: `is_fn` is `false` (component$ closures are not subject to loop hoisting)
    
*   An active hoisting scope exists: `hoisted_qrls` is non-empty
    

```js
// Inside .map(): each iteration creates a new closure with captured `item`
items.map((item) => (
  <button onPress$={handler}>  // handler captures item
    {item.name}
  </button>
));

// Without Level 2 hoisting: q_handler.w([item]) recreated every iteration
// With Level 2 hoisting: const q_h_0 = q_handler.w([item]) hoisted to outermost valid scope
```

#### compute\_hoist\_target\_depth Algorithm

`compute_hoist_target_depth` determines how far to hoist the `.w()` call:

*   **If** `scoped_idents` **is empty:** No runtime captures are needed. Hoist to the current component's top depth, taken from the `component_depths` stack. This places the const at the top of the component function body.
    
*   **If** `scoped_idents` **is non-empty:** Find `min_decl_scope` — the shallowest `decl_stack` level where ALL captured variables are declared. Then find the smallest entry `j` in `hoisting_scope_decl_indices` that covers `min_decl_scope`. Target depth = `min(j, current_depth)`.
    

The result: the `.w()` call is hoisted to the innermost scope that can see all captured variables, without escaping into a scope where those variables are undefined.

Source: `compute_hoist_target_depth` in `packages/optimizer/core/src/transform.rs` lines 1321–1370

#### hoisted\_qrls Stack

```plaintext
hoisted_qrls: Vec<Vec<(String, ast::VarDeclarator)>>
```

The `hoisted_qrls` field is a stack of hoisting scopes. Each element corresponds to one function or arrow expression scope. When a `.w()` call is hoisted, a `VarDeclarator` is stored at the target depth within this stack.

When a hoisting scope closes (the function or arrow expression finishes folding), `inject_hoisted_qrls_into_block` is called. It drains the accumulated `VarDeclarator` entries and emits `const` declarations at the **top** of that scope's body block — before any other statements. This ensures the hoisted QRL const is available for all code within the scope.

Source: `hoist_qrl_if_needed` in `packages/optimizer/core/src/transform.rs` lines 1376–1457; `compute_hoist_target_depth` lines 1321–1370; `inject_hoisted_qrls_into_block` in `transform.rs`

* * *

## Chapter 4: JSX Transform

### Overview

The JSX transform is the second major pass in `QwikTransform`. It runs after the QRL hoisting pass and processes every `jsx(type, props, key)` call that the upstream TypeScript JSX transform has produced from `<Component .../>` syntax. The entry point is `handle_jsx`.

The transform:

1.  Classifies the element type to determine whether it is a native element or a component (`is_fn`).
    
2.  Runs `handle_jsx_props_obj`, which splits props into a `varProps` bucket (dynamic) and a `constProps` bucket (static), extracts event handlers, handles `className → class`, expands `bind:value`/`bind:checked`, and injects `q:p`/`q:ps` capture props.
    
3.  Emits either `_jsxSplit` (runtime must sort props) or `_jsxSorted` (compile-time sorted) based on whether the props contain a spread or a component `bind:*` prop.
    

In Dev/Hmr mode, a 7th argument (`devLocation` object) is appended to the emitted call.

Source: `handle_jsx` in `packages/optimizer/core/src/transform.rs` lines 1147–1224

### handle\_jsx Entry Point

`handle_jsx` is called only when the callee is one of the known `jsx_functions` (imported from `@qwik.dev/core`, `@qwik.dev/core/jsx-runtime`, or `@qwik.dev/core/jsx-dev-runtime`, or originating from a `?jsx`/`.md` file).

#### Node Type Classification

The first step classifies the first argument (the element type):

| `node_type` form | `is_fn` | `is_text_only` | `jsx_mutable` |
| --- | --- | --- | --- |
| String literal (e.g., `"div"`) | `false` | result of `is_text_only(&str)` | unchanged |
| Identifier in `immutable_function_cmp` | `true` | `false` | unchanged |
| Identifier **not** in `immutable_function_cmp` | `true` | `false` | set to `true` |
| Any other expression | `true` | `false` | set to `true` |

`immutable_function_cmp` is the set of imported identifiers that are components with stable identity (e.g., those imported via `component$`/`component` imports). Any identifier **not** in this set is treated as potentially dynamic, which sets `jsx_mutable = true`.

#### Key Generation

A key is emitted when `should_emit_key = is_fn || self.root_jsx_mode`. The key format is `"<2-char-file-hash>_<counter>"` where:

*   The 2-char file hash is the first two characters of the base64-encoded `file_hash` field.
    
*   The counter is `jsx_key_counter`, incremented after each key emission.
    

`root_jsx_mode` is `true` for the outermost JSX expression in each subtree. It is set to `false` before recursing into children, then restored on exit. This ensures keys are only emitted at the outermost JSX call of each sub-tree, not for every nested node.

If `should_emit_key` is `false`, `null` is passed as the key argument.

#### Routing Decision

After calling `handle_jsx_props_obj` (which returns `(should_sort, var_props, const_props, children, flags)`):

| `should_sort` | Emitted call | Notes |
| --- | --- | --- |
| `true` | `_jsxSplit(type, varProps, constProps, children, flags, key)` | Runtime cannot rely on prop order; required when spreads or component `bind:*` props are present |
| `false` | `_jsxSorted(type, varProps, constProps, children, flags, key)` | Props are compile-time sorted alphabetically; runtime may apply optimizations |

In Dev/Hmr mode, a 7th argument is appended:

```typescript
// Dev mode only — 7th argument
_jsxSorted("div", null, { class: "foo" }, null, 3, "aB_0", {
  fileName: "src/app.tsx",
  lineNumber: 5,
  columnNumber: 3
});
```

Source: `handle_jsx` in `packages/optimizer/core/src/transform.rs` lines 1147–1224

### handle\_jsx\_props\_obj: Props Classification Pipeline

`handle_jsx_props_obj` is the outer wrapper around the classification pipeline. It:

1.  Delegates to `internal_handle_jsx_props_obj` to get `(should_sort, var_props_raw, const_props_raw, children, flags)`.
    
2.  Collapses empty `var_props_raw` to `null`; non-empty to an `ObjectLit`. A non-empty `var_props` also sets `jsx_mutable = true`.
    
3.  Collapses empty `const_props_raw` to `null`; non-empty to `build_unwrapped_props(const_props_raw)`.
    
4.  Builds the children argument via `build_children`.
    
5.  Builds the flags numeric literal via `build_flags`.
    

Source: `handle_jsx_props_obj` in `packages/optimizer/core/src/transform.rs` lines 2064–2103

### internal\_handle\_jsx\_props\_obj: The Pre-Scan and Main Loop

The internal function handles the actual prop classification. It operates in two phases: a **pre-scan** followed by the **main iteration loop**.

#### Phase 1: Pre-Scan

`const_idents` **computation:** All entries in `decl_stack` with type `IdentType::Var(true)` are collected into `const_idents`. These are module-scope `const` declarations initialized with constant expressions.

`last_spread_index` **detection:** The rightmost index of a `PropOrSpread::Spread` element in the props array, if any.

`has_var_prop_after_last_spread`**:** If `last_spread_index` is `Some(index)`, scan all props after that index. A prop is considered "var" (contributes `true`) unless it is:

*   A `children` prop (either shorthand or `key: value` form with key `"children"`) → `false`
    
*   A `KeyValue` prop whose value passes `is_const_expr` → `false`
    
*   Any other prop (including nested spreads) → `true`
    

If no spread exists, `has_var_prop_after_last_spread = false`.

\*\*OXC Pitfall 1 — Single-Pass Prop Processing:\*\* \`has\_var\_prop\_after\_last\_spread\` is a read-ahead pre-scan result. It \*\*must\*\* be computed before the main prop loop begins, not during it. If you process props in a single pass, you cannot correctly determine this value for the props you have not yet seen. Symptom: components with \`{...spread}\` followed by a static prop routing to \`\_jsxSorted\` when they should use \`\_jsxSplit\`.

`spread_props_count`**:** The total number of `PropOrSpread::Spread` entries in the prop list. This is a live counter, **decremented** in the main loop each time a spread is processed.

`should_runtime_sort` **(=** `has_spread_props || has_component_bind_props`**):**

*   `has_spread_props`: any spread exists in props (`spread_props_count > 0` before loop)
    
*   `has_component_bind_props`: `is_fn == true` AND at least one `bind:value` or `bind:checked` prop
    

This value is returned as `should_sort` and determines `_jsxSplit` vs `_jsxSorted`.

**Initial flag values:**

*   `static_listeners = !has_spread_props`
    
*   `static_subtree = !has_spread_props`
    
*   `moved_captures = false`
    

#### Phase 2: Main Loop

The main loop iterates over all props. At the start of each iteration:

```plaintext
is_target_const_props = (spread_props_count == 0)
```

This live check means:

*   All props **before** the first spread: `is_target_const_props = false` (a spread is still pending)
    
*   All props **after** the last spread is consumed: `is_target_const_props = true`
    

Each prop is dispatched to the appropriate handler based on its type (event, `className`, `bind:*`, children, spread, or regular).

**Sorting (when** `!should_runtime_sort`**):** After all props are processed, `var_props` is sorted alphabetically by key name. This is what makes `_jsxSorted` safe for runtime optimization.

Source: `internal_handle_jsx_props_obj` in `packages/optimizer/core/src/transform.rs` lines 2106–2653

### varProps vs constProps Classification Rules

For regular (non-event, non-children) props, `add_prop_to_appropriate_list` determines the target bucket:

```plaintext
if is_fn OR spread_props_count > 0:
    if is_const AND spread_props_count == 0 → const_props
    else → var_props
else:
    if NOT is_const OR spread_props_count > 0 → var_props
    else → const_props
```

In words: props that pass `is_const_expr` go to `const_props` when no pending spread exists. In component context (`is_fn = true`), const props can still go to `const_props` as long as the last spread has already been consumed.

#### `is_const_expr` Rules

Defined in `packages/optimizer/core/src/is_const.rs`. Starts with `is_const = true` and visits the expression tree. Any of the following sets `is_const = false`:

| Expression type | `is_const` result |
| --- | --- |
| Import reference (in `global_collect.imports`) | `true` |
| Export reference (in `global_collect` exports) | `true` |
| Identifier with `IdentType::Var(true)` in `const_idents` | `true` |
| Any call expression (`CallExpr`) | `false` |
| Any member access (`MemberExpr`) | `false` |
| Arrow function expression (`ArrowExpr`) | `true` (visitor does not descend) |
| Any other identifier not matched above | `false` |

`IdentType::Var(true)` means: a `const` declaration at module scope initialized with a constant expression (the `true` flag is set by the pre-transform constant analysis stage).

Source: `add_prop_to_appropriate_list` in `packages/optimizer/core/src/transform.rs` lines 3249–3295; `is_const_expr` in `packages/optimizer/core/src/is_const.rs` lines 12–74

### Flags Bitmask

The flags value is a `u32` bitmask with three bits:

| Bit | Name | Initial value | Cleared when |
| --- | --- | --- | --- |
| Bit 0 (`1 << 0`) | `static_listeners` | `!has_spread_props` | A QRL event handler is placed in `var_props` (not `const_props`) |
| Bit 1 (`1 << 1`) | `static_subtree` | `!has_spread_props` | `jsx_mutable` is set during children processing |
| Bit 2 (`1 << 2`) | `moved_captures` | `false` | Set to `true` when `q:p` or `q:ps` is injected |

The runtime uses the flags to skip unnecessary work:

*   `static_listeners` (`flags & 1`): all event handlers are in `constProps`; the runtime can cache them.
    
*   `static_subtree` (`flags & 2`): no dynamic children; the runtime can skip subtree diffing.
    
*   `moved_captures` (`flags & 4`): `q:p`/`q:ps` was injected; the runtime must read captures from the element.
    

```typescript
// flags = 3 means static_listeners=1, static_subtree=1, moved_captures=0
_jsxSorted("div", null, { class: "foo", "q-e:click": qrl(...) }, null, 3, "aB_0");

// flags = 4 means moved_captures=1 (q:p was injected)
_jsxSorted("div", { "q:p": item, "q-e:click": qrl(...) }, null, null, 4, "aB_2");
```

Source: `internal_handle_jsx_props_obj` in `packages/optimizer/core/src/transform.rs` lines 2613–2622

### Spread Props Handling

When a `{...obj}` spread is encountered and `obj` is a plain identifier, `handle_jsx_props_obj_spread` splits it into two runtime calls:

| Call | Destination | Condition |
| --- | --- | --- |
| `_getVarProps(obj)` | `var_props` | Always |
| `_getConstProps(obj)` | `const_props` | Only when: this is the last (or only) spread AND `!has_var_prop_after_last_spread` |
| `_getConstProps(obj)` | `var_props` | When condition above is not met |

When the spread expression is **not** a simple identifier (e.g., a computed expression), the raw spread is added to `var_props` after folding.

**Before / after examples:**

```typescript
// Input: <div class="foo" onClick$={handler} />
// (no spreads, all const props — flags=3: static_listeners|static_subtree)
_jsxSorted("div", null, { class: "foo", "q-e:click": qrl(...) }, null, 3, "aB_0");

// Input: <div {...props} id="fixed" />
// (spread exists — _jsxSplit, flags=0)
_jsxSplit("div",
  { ..._getVarProps(props) },
  { ..._getConstProps(props), id: "fixed" },
  null, 0, "aB_1");

// Input: <div {...props} id={dynamicId} />
// (spread followed by var prop — has_var_prop_after_last_spread=true)
// _getConstProps goes to var_props because constProps promotion blocked
_jsxSplit("div",
  { ..._getVarProps(props), ..._getConstProps(props), id: dynamicId },
  null,
  null, 0, "aB_2");
```

Source: `handle_jsx_props_obj_spread` in `packages/optimizer/core/src/transform.rs` lines 2655–2711

### Event Handler Extraction

`jsx_event_to_html_attribute` converts JSX event prop names to HTML data attributes. This conversion only applies to **native elements** (`is_fn == false`).

#### Event Name Translation Table

| JSX prop prefix | Output prefix | Trailing `$` stripped | Example |
| --- | --- | --- | --- |
| `on` | `q-e:` | Yes | `onClick$` → `q-e:click` |
| `window:on` | `q-w:` | Yes | `window:onScroll$` → `q-w:scroll` |
| `document:on` | `q-d:` | Yes | `document:onLoad$` → `q-d:load` |
| None of the above | (returns `None`) | — | Not an event prop |

The function requires the prop name to end with `$`. Any name not ending with `$` returns `None`.

**camelCase to kebab-case:** Each uppercase ASCII letter in the event name becomes `-<lowercase>`. For example, `onClick$` → event name `Click` → `click`; `onKeyDown$` → event name `KeyDown` → `key-down`.

**Case-sensitive events:** If the event name after stripping the scope prefix starts with `-`, the `-` is removed and the name is **not** lowercased. This preserves case for custom events that require exact casing.

**Special case:** `DOMContentLoaded` maps to `d-o-m-content-loaded` (each uppercase letter produces `-<lowercase>`). For `document:onDOMContentLoaded$`, the output is `q-d:d-o-m-content-loaded`.

Source: `jsx_event_to_html_attribute` and `get_event_scope_data_from_jsx_event` and `create_event_name` in `packages/optimizer/core/src/transform.rs` lines 4637–4694

### className → class Rename

For native elements (`is_fn == false`), if the prop key is `className`, it is renamed to `class`. Specifically, `transformed_event_key` is set to `CLASS` (the atom `"class"`) so the output prop uses `"class"` as a `PropName::Str` key.

This rename applies regardless of whether `className` has a const or var value.

Source: `transform_jsx_prop` in `packages/optimizer/core/src/transform.rs` lines 1771–1777

### bind:value / bind:checked Two-Step Expansion

For native elements, `bind:value` and `bind:checked` props are expanded into two props: a value/checked accessor and a `q-e:input` handler. The expansion depends on three conditions:

| Context | `should_sort` | `is_target_const_props` | Behavior |
| --- | --- | --- | --- |
| `_jsxSplit` (spreads or component `bind:*`) | `true` | any | **Skip** — return without expanding; `bind:*` stays in `var_props` for runtime handling |
| `_jsxSorted`, varProps target | `false` | `false` | Expand into `var_props` |
| constProps target | any | `true` | Always expand; emit into `const_props` |

The check order (from `transform_jsx_prop` lines 1783–1788) is:

1.  If `should_sort && is_bind_prop(kw)` → return early without expansion (`_jsxSplit` case).
    
2.  If `is_target_const_props || !should_sort` → expand.
    

**Expansion creates two props:**

1.  A `value` or `checked` prop pointing to the signal expression.
    
2.  A `q-e:input` event handler: `inlinedQrl(_val, "_val", [signal])` (for `bind:value`) or `inlinedQrl(_chk, "_chk", [signal])` (for `bind:checked`).
    

The `q-e:input` handler is merged with any existing `q-e:input` handler via `merge_or_add_event_handler`.

```typescript
// Input: <input bind:value={signal} />
// Output (_jsxSorted, no spreads):
_jsxSorted("input", null, {
  value: signal,
  "q-e:input": inlinedQrl(_val, "_val", [signal])
}, null, 3, "aB_5");
```

\*\*OXC Pitfall 2 — bind:\* Routing Confusion:\*\* The three-branch expansion rule is easy to conflate. The key guard is \`should\_sort && is\_bind\_prop(kw)\` at line 1784: if \`should\_runtime\_sort\` is \`true\` (i.e., spreads or component \`bind:\*\` exist), skip expansion entirely and leave \`bind:\*\` in \`var\_props\`. Expanding \`bind:value\` in \`\_jsxSplit\` mode produces incorrect output — the runtime expects to find the raw \`bind:value\` prop and handle it itself.

Source: `transform_jsx_prop` in `packages/optimizer/core/src/transform.rs` lines 1748–1865

### q:p / q:ps Native Element Capture Injection

For native elements (`is_fn == false`), event handlers that close over local variables need those captures serialized as element props. The optimizer injects either `q:p` (single capture) or `q:ps` (multiple captures) into `var_props`.

#### element\_lifted\_params Pre-Pass

Before the main prop loop, `element_lifted_params` is computed. This is the list of identifiers to inject. Two cases:

**Case 1 — Loop context** (`iteration_var_stack` is non-empty): Collect the subset of iteration variables (from the innermost `iteration_var_stack` entry) that are actually referenced by any event handler prop (a `KeyValue` prop whose key converts via `convert_qrl_word` and whose value is an arrow or function expression). Only iteration variables referenced by handlers are collected.

**Case 2 — No loop context** (`iteration_var_stack` is empty): Call `compute_handler_captures` on every event handler prop and take the union of all returned capture sets, deduplicated.

#### compute\_handler\_captures Algorithm

For a given handler expression:

1.  Collect all identifiers referenced in the handler body.
    
2.  Partition `decl_stack` entries into `Var`\-typed (`IdentType::Var(_)`) vs. other.
    
3.  Call `compute_scoped_idents` to find the intersection of handler identifiers with `Var`\-typed declarations.
    
4.  Remove the handler's own parameters from the result.
    
5.  Return the remaining scoped identifiers — these are the runtime captures needed.
    

Source: `compute_handler_captures` in `packages/optimizer/core/src/transform.rs` lines 1286–1308

#### Injection Rules

Injection happens during the main loop when an event handler prop is processed:

```plaintext
if params_to_lift is non-empty AND !is_fn AND !moved_captures:
    if params_to_lift.len() == 1:
        push { "q:p": ident } to var_props
    else:
        push { "q:ps": [ident0, ident1, ...] } to var_props
    moved_captures = true
```

*   `moved_captures` acts as a guard — it is set to `true` after the first injection and prevents duplicate `q:p`/`q:ps` props on the same element.
    
*   Both `q:p` and `q:ps` are always pushed to `var_props` (never `const_props`).
    

```typescript
// Input: items.map(item => <div onClick$={() => handle(item)} />)
// Output per item (single capture):
_jsxSorted("div", { "q:p": item, "q-e:click": qrl(...) }, null, null, 4, "aB_2");
//                  ^-- single capture: q:p with identifier directly
//  flags=4: moved_captures bit set

// Input: items.map((item, key) => <div onClick$={() => handle(item, key)} />)
// Output per item (multiple captures):
_jsxSorted("div", { "q:ps": [item, key], "q-e:click": qrl(...) }, null, null, 4, "aB_3");
```

\*\*OXC Pitfall 4 — q:p Injected Multiple Times:\*\* If a native element has multiple event handlers, each handler triggers the capture injection check. Without the \`moved\_captures\` guard, each handler would push a separate \`q:p\` or \`q:ps\` prop. Implement \`moved\_captures\` as a boolean flag, set it to \`true\` on first injection, and guard all subsequent injection attempts with \`!moved\_captures\`. A single element must never have more than one \`q:p\` or \`q:ps\` prop.

Source: `element_lifted_params` computation in `packages/optimizer/core/src/transform.rs` lines 2195–2265; injection in lines 2432–2476

* * *

### \_fnSignal / \_wrapProp Inline Signal Wrapping

After `q:p`/`q:ps` injection, the optimizer processes non-QRL, non-children props whose values are non-const expressions. For each such prop, `convert_to_getter` calls `create_synthetic_qqsegment` to determine whether the expression can be wrapped as a live signal getter. If it can, the prop value is replaced with a `_wrapProp` or `_fnSignal` call that the runtime re-evaluates whenever the captured signals change.

This wrapping is what enables fine-grained DOM updates: instead of re-rendering the component, the runtime calls the getter and patches only the affected attribute.

Source: `convert_to_getter` in `packages/optimizer/core/src/transform.rs` (calls `create_synthetic_qqsegment`); also used by `convert_to_signal_item` for array children.

### create\_synthetic\_qqsegment Decision Tree

The entry function `create_synthetic_qqsegment` decides whether an expression is eligible for signal wrapping and, if so, which form to use.

**Algorithm (source:** `create_synthetic_qqsegment` **in** `packages/optimizer/core/src/transform.rs` **lines 737–818):**

1.  Collect all identifiers referenced in the expression (`descendent_idents`).
    
2.  Partition `decl_stack` entries into `decl_collect` (`IdentType::Var(_)` only) vs `invalid_decl` (all other declaration types: function params, class names, import bindings, etc.).
    
3.  If any `descendent_ident` is in `invalid_decl` → return `(None, false)`. The expression references a non-variable declaration; wrapping would break semantics.
    
4.  For each identifier not in `decl_collect`: if it is a global known to have side effects (`options.global_collect.is_global(ident)`) → set `contains_side_effect = true`.
    
5.  Call `compute_scoped_idents(&descendent_idents, &decl_collect)` → `(scoped_idents, is_const)`.
    
6.  If `contains_side_effect` → return `(None, scoped_idents.is_empty())`. Side-effecting expressions cannot be wrapped, but `is_const` is set `true` if no captures exist (empty scoped\_idents → no lazy evaluation needed).
    
7.  If expression is a plain `Ident` → return `(None, is_const)`. Simple variable references need no wrapping.
    
8.  If `!is_const` AND expression is a `Call` or `Tpl` (template literal) → return `(None, false)`. Non-const calls and template literals cannot be safely serialized for re-evaluation.
    

If all eight checks pass, the function proceeds to `_wrapProp` or `convert_inlined_fn`.

### \_wrapProp Optimization

Before delegating to `convert_inlined_fn`, `create_synthetic_qqsegment` applies a specialized fast path for the most common signal access pattern: a single-level member expression where the object is a plain identifier.

**Trigger (source:** `transform.rs` **lines 791–806):**

```plaintext
expression form:  ident.prop
                  OR (ident as Type).prop  (parenthesized ident)
```

If the expression is a `Member` expression whose (unwrapped) object is an `Ident`, and the property name can be extracted as a string, the optimizer emits:

```typescript
_wrapProp(ident, "prop")
```

instead of going through `convert_inlined_fn`. This is a lightweight specialization — `_wrapProp` is a two-argument call requiring no arrow function construction or scope capture array.

**Non-matching cases:** Multi-level member access (`a.b.c`), non-identifier object expressions (e.g., function calls as object), or properties that cannot be represented as plain strings all fall through to `convert_inlined_fn`.

```typescript
// Source: create_synthetic_qqsegment in transform.rs lines 791–806
// Input:  <div style={signal.value} />
// Output: _wrapProp(signal, "value")
_jsxSorted("div", null, { style: _wrapProp(signal, "value") }, null, 3, "aB_3");

// Input:  <div style={a.b.value} />   (multi-level — falls through to convert_inlined_fn)
// No _wrapProp here
```

### convert\_inlined\_fn Eligibility

For expressions that are not simple idents or single-level member accesses, `create_synthetic_qqsegment` delegates to `convert_inlined_fn` in `inlined_fn.rs`. This function applies six ordered eligibility checks, each returning early if the expression cannot be wrapped.

**Eligibility checks (source:** `convert_inlined_fn` **in** `packages/optimizer/core/src/inlined_fn.rs` **lines 24–115):**

| # | Check | Return if triggered | Notes |
| --- | --- | --- | --- |
| 1 | Expression is `ArrowExpr` | `(None, is_const)` | Arrow exprs are already QRL boundaries; no wrapping needed |
| 2 | `used_as_call = true` | `(None, false)` | Expression contains a call; cannot serialize for re-evaluation |
| 3 | `!used_as_object` | `(None, is_const)` | No captured identifier appears as object of member access or in logical OR; wrapping provides no benefit |
| 4 | `abort` flag set by `ReplaceIdentifiers` | `(None, is_const)` | Contains nested arrow, function, class, decorator, or statement |
| 5 | Rendered expression length > 150 chars | `(None, false)` | Too large to inline; cannot guarantee const-ness without wrapping |
| 6 | `scoped_idents.is_empty()` | `(None, true)` | No captures; expression is constant, no lazy evaluation needed |

`used_as_object` **check:** The `ObjectUsageChecker` visitor traverses the expression looking for a captured identifier (`scoped_idents` member) that appears as:

*   The object of a member expression (`.prop` access), or
    
*   An operand in a logical OR (`||`) expression (recursively checked).
    

Pure function calls involving captures (`capturedFn()`) set `used_as_call = true` and fail check 2 before `used_as_object` can pass.

**If all six checks pass**, `convert_inlined_fn` emits:

```typescript
_fnSignal((p0, p1, ...) => <expr with pN replacing captured idents>, [cap0, cap1, ...])
```

In server mode (`is_server = true`), a third argument is added — the stringified source expression:

```typescript
_fnSignal((p0) => `${p0} items`, [count], "(p0) => `${p0} items`")
//                                          ^-- third arg: source string for SSR serialization
```

```typescript
// Source: convert_inlined_fn in inlined_fn.rs lines 82–114; hoist_fn_signal_call in transform.rs
// Input:  <div title={`${count.value} items`} />
// Output (client mode):
_jsxSorted("div", null, { title: _fnSignal((p0) => `${p0} items`, [count]) }, null, 3, "aB_4");

// Output (server mode, is_server = true):
_jsxSorted("div", null, { title: _fnSignal((p0) => `${p0} items`, [count], "(p0)=>`${p0} items`") }, null, 3, "aB_4");
```

\*\*OXC Pitfall 3 — \_fnSignal with Call Expressions:\*\* The \`used\_as\_call\` check in \`inlined\_fn.rs\` must be implemented unconditionally via the \`ObjectUsageChecker::visit\_call\_expr\` visitor, which sets \`used\_as\_call = true\` for any \`CallExpr\` node anywhere in the expression tree. If this check is omitted, expressions like \`signal.doSomething()\` may be incorrectly wrapped in \`\_fnSignal\`, causing serialization failures at runtime because function calls cannot be safely re-evaluated from serialized state.

### hoist\_fn\_signal\_call Deduplication

When the same `_fnSignal` arrow function appears in multiple JSX props (e.g., a reactive expression used in both `style` and `aria-label`), the optimizer deduplicates them by hoisting the arrow function to module scope.

**Algorithm (source:** `hoist_fn_signal_call` **in** `packages/optimizer/core/src/transform.rs` **lines 2763–2872):**

1.  Render the first argument (the arrow function) to a canonical string: `fn_body_str = render_expr(arrow_expr)`.
    
2.  Look up `hoisted_fn_signals` HashMap (keyed by `fn_body_str`).
    
3.  **If found:** Replace the arrow argument with an ident reference to the existing hoisted const. If the call has a third argument (server-mode string literal), replace it with a reference to the corresponding `_hf<N>_str` ident.
    
4.  **If not found:**
    
    *   Allocate `fn_name = "_hf<hoisted_fn_counter>"` and increment `hoisted_fn_counter`.
        
    *   Create `const _hf<N> = <arrow_expr>;` and insert into `extra_top_items` (module-scope items added before the transformed module body).
        
    *   Register `fn_body_str → fn_id` in `hoisted_fn_signals`.
        
    *   If server mode (third arg is a string literal): also create `const _hf<N>_str = "<source>";` and insert into `extra_top_items`.
        
    *   Replace arrow argument with ident reference to `_hf<N>`.
        

This deduplication also runs in `fold_call_expr` when the AST already contains `_fnSignal` calls (e.g., from a prior transform pass). Any `_fnSignal(arrow, ...)` encountered during folding is routed through `hoist_fn_signal_call` (source: `transform.rs` lines 3993–4035).

```typescript
// Before hoisting (two identical signal expressions):
_jsxSorted("div", null, {
  style: _fnSignal((p0) => p0.color, [theme]),
  color: _fnSignal((p0) => p0.color, [theme]),
}, null, 3, "aB_5");

// After hoist_fn_signal_call deduplication:
const _hf0 = (p0) => p0.color;
_jsxSorted("div", null, {
  style: _fnSignal(_hf0, [theme]),
  color: _fnSignal(_hf0, [theme]),
}, null, 3, "aB_5");
```

### \_wrapProp vs \_fnSignal Summary Table

| Situation | Output form | Notes |
| --- | --- | --- |
| `signal.value` — single-level `ident.prop` | `_wrapProp(signal, "value")` | Fast path; no arrow function construction |
| `(expr as T).prop` where `expr` is an ident | `_wrapProp(expr, "prop")` | Parenthesized ident unwrapped before check |
| `a.b.c` — multi-level member access | not wrapped (falls through; `used_as_object` must pass) | Falls through to `convert_inlined_fn`; eligible only if captures pass all 6 checks |
| General expression with captured vars | `_fnSignal((p0, ...) => expr, [cap, ...])` | Arrow args hoisted to `_hf<N>` by `hoist_fn_signal_call` |
| Constant expression (no captures) | expression used as-is | `is_const = true`; no wrapping |
| Expression with side-effectful global | not wrapped | `contains_side_effect` check returns early |

* * *

### EachTransform: .map() → Each Rewrite

> **Status: Planned Addition — draft PR #8482.** The current `build/v2` branch has no automatic `.map()` → `<Each>` rewrite. `.map()` in JSX children is passed through unchanged, with the `q:p`/`q:ps` capture injection documented above as the only optimizer intervention. This section documents the planned behavior from draft PR `#8482` (`feat(optimizer): automatic loop optimization`).

### Current State

On the current `build/v2` branch, the optimizer has no `each_transform.rs` module and makes no attempt to rewrite `.map()` calls. The only optimizer-level processing that touches `.map()` children is the capture injection described in the `q:p`/`q:ps` section above.

### Planned State (PR #8482)

Draft PR `#8482` adds `each_transform.rs` as a new module. The entry function `try_rewrite_map_to_each` is called from `fold_call_expr` whenever a call expression is encountered inside a JSX children context (`jsx_children_expr_depth > 0`). The function inspects whether the call is a `.map()` on an array and, if so, attempts to replace it with an `<Each>` component usage.

### Entry Guards

The rewrite is skipped (silently, no warning) if any of the following guards fires:

| Guard | Condition to skip | Notes |
| --- | --- | --- |
| JSX children depth | `jsx_children_expr_depth == 0` | Not inside JSX children; counter tracks nesting depth |
| Entry strategy | `EntryStrategy::Hoist` | Noop QRLs produced by Hoist strategy are incompatible with `Each` resumability |
| Opt-out directive | `has_disabled_optimizer_rule(span, "map-to-each")` | `/* @qwik-disable-next-line map-to-each */` comment on the expression suppresses both the rewrite and all warnings |
| Callee shape | Not an `expr.map(...)` call pattern | Must be a method call named `map` |
| Callback shape | Not an arrow or function expression, or zero parameters | Callback must have at least one parameter (the item) |

An additional guard applies after the callback shape check:

`item_expr_has_dynamic_component_reference`**:** If the callback item expression contains a component reference that is dynamically determined by the callback parameter (e.g., `items.map(item => <item.Component />)`), the rewrite is skipped with **no warning**. This guard prevents incorrect rewrites when each item renders a different component type.

### Key Extraction Rules

The key expression is extracted from the JSX node returned by the callback. Two modes are supported based on `transpile_jsx`:

| Mode | `transpile_jsx` | Key extraction function |
| --- | --- | --- |
| Raw JSX | `false` | `remove_key_from_jsx_element` — reads `key` attribute directly from JSX element |
| Lowered JSX calls | `true` | `remove_key_from_transpiled_jsx_call` — extracts third argument from `jsx(type, props, key)` call |

If the key cannot be cleanly extracted, the rewrite is aborted with a diagnostic warning. The four key disqualifiers are:

| Disqualifier | Warning code | Trigger condition |
| --- | --- | --- |
| `MissingKey` | `"map-to-each"` | No `key` prop on the returned JSX node |
| `UsesSecondParamForKey` | `"map-to-each"` | Key expression references the callback's second parameter (the array index) |
| `CallDerivedKey` | `"map-to-each"` | Key expression contains a function call expression |
| `NotSingleJsxNode` | `"map-to-each"` | Callback does not return a single JSX element (e.g., returns a fragment or conditional) |

### Shared Alias Handling

For callbacks with a block body (statements before the return), `slice_statements_for_target` computes the minimal subset of statements needed to evaluate either the `key$` or `item$` expression in isolation. Simple `const` aliases used by both `key$` and `item$` may appear in both generated arrow functions.

If `slice_statements_for_target` cannot produce a valid slice (e.g., due to complex control flow), it returns an `EachSliceError`. This error is mapped to a `"map-to-each"` diagnostic warning and the rewrite is aborted — the original `.map()` call is emitted unchanged.

### Two Output Paths

The output form depends on whether the callback captures any identifiers from the outer scope:

| Captured scope identifiers | Output form | Notes |
| --- | --- | --- |
| None | Direct `<Each>` component rewrite | `<Each items={...} key$={...} item$={...} />` |
| Some | `_map(items, fallbackFn, key$QRL, item$QRL, [captures])` helper call | Runtime decides between `Each` keyed path and original `.map()` based on capture serializability |

For the captured path, `key$` and `item$` QRL arguments are produced using `create_deferred_capture_qsegment` — a new helper introduced in PR `#8482` that is not present in the current `build/v2` codebase. The `_map` helper (defined in `words.rs` in the PR) makes the final routing decision at runtime.

```typescript
// Source: each_transform.rs in draft PR #8482
// Input (no captured scope):
items.map(item => <Card key={item.id} item={item} />)
// Output:
<Each items={items} key$={(item) => item.id} item$={(item) => <Card item={item} />} />

// Input (captured scope ident: `theme`):
items.map(item => <Card key={item.id} theme={theme} />)
// Output:
_map(items, (item) => <Card key={item.id} theme={theme} />, key$QRL, item$QRL, [theme])
```

### Diagnostic Warnings

All warnings produced by the `EachTransform` use the warning code `"map-to-each"` (constant `MAP_TO_EACH_DIRECTIVE`). Diagnostics use:

*   `DiagnosticCategory::Warning`
    
*   `DiagnosticScope::Optimizer`
    

The `/* @qwik-disable-next-line map-to-each */` opt-out directive suppresses both the rewrite **and** all associated warnings for the annotated `.map()` call. This allows developers to keep manual `.map()` patterns without diagnostic noise.

When PR \`#8482\` merges, the pipeline stage count documented in Chapter 2 will increase by one (the \`EachTransform\` stage). Update the stage table in Chapter 2 and this section accordingly. The \`jsx\_children\_expr\_depth\` counter sites (increment on JSX children entry, decrement on exit) are in the restructured \`transform/mod.rs\` introduced by the PR and are not visible in the current \`build/v2\` branch.

Source: `try_rewrite_map_to_each`, `analyze_each_callback`, `push_each_candidate_warning` in `each_transform.rs` from draft PR `#8482` (`gh pr diff 8482 --repo BuilderIO/qwik`); `MAP_TO_EACH_DIRECTIVE` constant and `EachSliceError` enum from same source. Not present in current `build/v2`.

* * *

## Chapter 5: Segment Module Generation, Post-Processing, and Output

Chapter 5 covers three sub-systems that execute **after** `QwikTransform` has extracted all segments from the source file but **before** the final output is assembled:

1.  **Post-transform passes** — Two independent DCE branches run on the root module. They are mutually exclusive per strategy branch.
    
2.  **Variable migration pipeline** — Root-level variables used exclusively by a single segment are migrated into that segment's module, shrinking the root module.
    
3.  **Segment module construction** (`new_module`) and **manifest output** — Each extracted segment becomes a standalone AST module; `get_manifest()` assembles the `QwikManifest`. *(Covered in Chapter 5 Part 2.)*
    

**Execution order within** `transform_code` (in `parse.rs`):

```plaintext
1. QwikTransform folds the root module → segments extracted
2. Post-transform passes run on root module (Treeshaker OR SideEffectVisitor)
3. apply_variable_migration populates segment.data.migrated_root_vars
4. remove_migrated_exports / remove_unused_qrl_declarations clean up root module
5. Optional re-DCE if migration happened
6. new_module() constructs each segment's standalone AST
7. get_manifest() assembles QwikManifest from all TransformModules
```

* * *

### Post-Transform Passes

Two independent DCE branches operate on the root module after `QwikTransform` completes. They are **mutually exclusive per strategy**: for `Inline`/`Hoist` strategies the `SideEffectVisitor` runs; for all other strategies with minify enabled the `Treeshaker` runs. This branching is visible in the `transform_code` function in `parse.rs` (~line 372):

```rust
// Simplified from parse.rs transform_code
if matches!(entry_strategy, EntryStrategy::Inline | EntryStrategy::Hoist) {
    program.visit_mut_with(&mut SideEffectVisitor::new(...));
} else if config.minify != MinifyMode::None && !config.is_server {
    program.visit_mut_with(&mut treeshaker.cleaner);
    if treeshaker.cleaner.did_drop {
        program.mutate(&mut simplify::simplifier(...));
    }
}
```

\*\*Pitfall 6:\*\* Never apply both \`SideEffectVisitor\` AND \`CleanSideEffects\` to the same module. They run in mutually exclusive branches: \`Inline\`/\`Hoist\` → \`SideEffectVisitor\`; all other strategies with minify → \`Treeshaker\`. Applying both will incorrectly remove imports that \`SideEffectVisitor\` just injected.

* * *

### Pass 1: Treeshaker (Client-Side DCE)

**Gate:** `config.minify != MinifyMode::None && !config.is_server`

**NOT for:** `EmitMode::Lib` (the outer `if config.mode != EmitMode::Lib` block excludes it).

The `Treeshaker` struct (defined in `clean_side_effects.rs`) owns two sub-visitors that share a single `Rc<RefCell<HashSet<Span>>>`:

```rust
pub struct Treeshaker {
    pub marker: CleanMarker,
    pub cleaner: CleanSideEffects,
}
pub struct CleanMarker   { set: Rc<RefCell<HashSet<Span>>> }
pub struct CleanSideEffects { pub did_drop: bool, set: Rc<RefCell<HashSet<Span>>> }
```

**Two-pass algorithm:**

| Pass | Visitor | Runs | What it does |
| --- | --- | --- | --- |
| 1 | `CleanMarker` | BEFORE `simplify` | Marks spans of all top-level `new` and `call` expression statements that exist in the pre-simplify AST |
| — | SWC `simplify` | Between passes | Standard dead-code elimination — removes unreferenced variables, dead branches |
| 2 | `CleanSideEffects` | AFTER `simplify` | Drops any top-level `new`/`call` expression statement whose span was NOT in the pre-simplify set |
| 3 (optional) | SWC `simplify` | Only if `did_drop == true` | Second DCE pass to clean up now-orphaned import declarations |

The key invariant: `CleanMarker` records which side-effecting expressions the **user wrote**. After `simplify` runs, any `new`/`call` statement that was NOT in the pre-simplify set was introduced by the transform (e.g., a `qrl(...)` call left as a standalone statement after its variable binding was DCE'd). `CleanSideEffects` removes those transform-introduced side effects.

```rust
// clean_side_effects.rs — CleanMarker (lines 51-65)
impl VisitMut for CleanMarker {
    fn visit_mut_module_item(&mut self, node: &mut ModuleItem) {
        if let ModuleItem::Stmt(Stmt::Expr(expr)) = node {
            match &*expr.expr {
                Expr::New(e) => { self.set.borrow_mut().insert(e.span()); }
                Expr::Call(e) => { self.set.borrow_mut().insert(e.span()); }
                _ => {}
            }
        }
    }
}

// CleanSideEffects (lines 67-90)
impl VisitMut for CleanSideEffects {
    fn visit_mut_module(&mut self, node: &mut Module) {
        node.body.retain(|item| match item {
            ModuleItem::Stmt(Stmt::Expr(expr)) => match &*expr.expr {
                Expr::New(e) => {
                    if self.set.borrow().contains(&e.span()) { return true; }
                    self.did_drop = true;
                    false
                }
                Expr::Call(e) => {
                    if self.set.borrow().contains(&e.span()) { return true; }
                    self.did_drop = true;
                    false
                }
                _ => true,
            },
            _ => true,
        });
    }
}
```

`did_drop` **flag:** When `CleanSideEffects` removes at least one item it sets `did_drop = true`. The caller (`transform_code`) checks this flag and runs a second `simplify` pass only when needed, avoiding unnecessary work.

\*\*Pitfall 5:\*\* The \`CleanMarker\` visitor is only executed when \`!config.is\_server\`. On server builds the Treeshaker is entirely skipped — side effects like route loader registrations must be preserved. Do not apply \`CleanSideEffects\` on server bundles.

Source: `Treeshaker`, `CleanMarker`, `CleanSideEffects` in `clean_side_effects.rs` (lines 1–90); call site in `parse.rs` `transform_code` (~lines 352–398).

* * *

### Pass 2: SideEffectVisitor (Side-Effect Import Injection)

**Gate:** `matches!(entry_strategy, EntryStrategy::Inline | EntryStrategy::Hoist)`

**Regardless of:** `minify` setting, `is_server` flag, or `EmitMode`.

`SideEffectVisitor` (in `add_side_effect.rs`) ensures that relative imports referenced by `global_collect.imports` are not silently tree-shaken when using `Inline` or `Hoist` strategies. In those strategies, segments stay in the same file — tree-shaking may remove an import that a segment indirectly depends on.

**Two-step algorithm (both steps happen inside** `visit_mut_module`**):**

```plaintext
Step 1 — Collect existing relative imports from module body:
    for each ImportDecl in module.body:
        if src.starts_with('.'): self.imports.insert(src)

Step 2 — Inject missing side-effect imports:
    let mut sorted = global_collect.imports.values().sorted_by(|i| i.source)
    for import in sorted:
        if import.source.starts_with('.')
           AND !self.imports.contains(import.source)
           AND normalize(abs_dir + import.source).starts_with(src_dir):
            module.body.insert(0, ImportDecl { specifiers: [], src: import.source })
```

The bare import (`import './module'`) is inserted at **position 0** — prepended before all other module items. This ensures side-effect imports execute before any other code.

`src_dir` **boundary check:** Only imports that resolve to a path inside `src_dir` are injected. This prevents injecting bare imports for `node_modules` paths that happen to appear in `global_collect.imports`.

```rust
// add_side_effect.rs — SideEffectVisitor (lines 34-66)
impl VisitMut for SideEffectVisitor {
    fn visit_mut_import_decl(&mut self, node: &mut ast::ImportDecl) {
        if node.src.value.starts_with('.') {
            self.imports.insert(node.src.value.clone());
        }
    }
    fn visit_mut_module(&mut self, node: &mut ast::Module) {
        node.visit_mut_children_with(self);  // runs visit_mut_import_decl for each import
        let mut imports: Vec<_> = self.global_collector.imports.values().collect();
        imports.sort_by_key(|i| i.source.clone());
        for import in imports {
            if import.source.starts_with('.') && !self.imports.contains(&import.source) {
                let abs_dir = self.path_data.abs_dir.to_slash_lossy();
                let relative = relative_path::RelativePath::new(&abs_dir);
                let final_path = relative.join(import.source.as_ref()).normalize();
                if final_path.starts_with(self.src_dir.to_str().unwrap()) {
                    node.body.insert(0, ast::ModuleItem::ModuleDecl(
                        ast::ModuleDecl::Import(ast::ImportDecl {
                            specifiers: vec![],
                            src: Box::new(ast::Str::from(import.source.clone())),
                            ..
                        })
                    ));
                }
            }
        }
    }
}
```

Source: `SideEffectVisitor` in `add_side_effect.rs` (lines 12–66); call site in `parse.rs` `transform_code` (~lines 372–379).

* * *

### Variable Migration Pipeline

After segment extraction, root-level variables that are used **exclusively** by a single segment (not user-exported, not referenced by the root module at runtime, not themselves imports) migrate into that segment's module. This reduces the root module footprint and improves tree-shaking in the final bundle.

**Gate:** `config.mode != EmitMode::Lib && !segments.is_empty()`

The pipeline is implemented in `apply_variable_migration` (`parse.rs`, ~line 982) and calls into `dependency_analysis.rs`. The full 10-step flow:

```plaintext
Steps 1–4: Analysis phase (read-only)
  1. analyze_root_dependencies   → HashMap<Id, RootVarDependency>
  2. build_root_var_usage_map    → IndexMap<Id, Vec<usize>>  (which segments use each var)
  3. build_main_module_usage_set → HashSet<Id>               (vars still live in root)
  4. find_migratable_vars        → BTreeMap<usize, Vec<Id>>  (candidates per segment)

Step 5: Pre-declare auto-exports needed after migration
  5. precompute_and_declare_auto_exports (mutates module + global_collect)

Steps 6–7: Populate and strip
  6. Topological sort + cyclic split + dedup → segment.data.migrated_root_vars
  7. local_idents.retain / scoped_idents.retain (strip migrated vars)

Steps 8–10: Root module cleanup
  8. remove_migrated_exports      (removes ExportNamed specifiers + Decl stmts)
  9. remove_unused_qrl_declarations (iterative _qrl_*/i_* cleanup)
 10. Optional re-DCE if migrated && minify != None
```

* * *

#### Step 1: `analyze_root_dependencies`

**Source:** `dependency_analysis.rs`, `analyze_root_dependencies` function (lines 28–273).

Returns `HashMap<Id, RootVarDependency>` for every root-level declaration in the module.

```rust
pub enum RootVarDecl {
    Var(ast::VarDeclarator),
    Fn(ast::FnDecl),
    Class(ast::ClassDecl),
    TsEnum(Box<ast::TsEnumDecl>),
}

pub struct RootVarDependency {
    pub decl:        RootVarDecl,
    pub is_imported: bool,   // true if this id comes from global.imports
    pub is_exported: bool,   // true if user-exported (NOT _auto_ exports)
    pub depends_on:  Vec<Id>, // identifiers referenced in the initializer/body
}
```

**Two-pass construction:**

*   **Pre-scan (user\_exported set):** Walk module body collecting all export declarations. `_auto_`\-prefixed export aliases are explicitly **skipped** — they are auto-generated by `ensure_export` and do not indicate user intent to export.
    
*   **Pass 1 (stub entries):** Walk `module.body` collecting all `Decl` items (Var/Fn/Class/TsEnum) and insert stub `RootVarDependency` entries into the map. Also adds entries from `global_collect.root` (import bindings).
    
*   **Pass 2 (dependency extraction):** For each `Var` decl with an initializer, run `IdentCollector` on the init expression and populate `depends_on`.
    

\*\*Pitfall 7 — \`\_auto\_\` prefix convention:\*\* \`analyze\_root\_dependencies\` sets \`is\_exported = false\` for variables that appear only in \`\_auto\_\`-prefixed export aliases. This is intentional: \`\_auto\_\` exports are internal plumbing added by \`ensure\_export\` (see Chapter 3) so that segment modules can import root-level identifiers. They must not block migration. The check is: \`let is\_auto\_export = exported\_id.sym.starts\_with("\_auto\_")\` (\`dependency\_analysis.rs\` line 71).

* * *

#### Steps 2–3: Usage Maps

**Step 2 —** `build_root_var_usage_map` (`dependency_analysis.rs`, line 313):

Returns `IndexMap<Id, Vec<usize>>` — for each root variable id, which segment indices use it. Checks both `local_idents` and `scoped_idents` of each segment. Only tracks ids that appear in `root_dependencies` (i.e., skips external imports).

```rust
for (seg_idx, segment) in segments.iter().enumerate() {
    let all_idents = [segment.data.local_idents.clone(),
                      segment.data.scoped_idents.clone()].concat();
    for local_id in &all_idents {
        if root_dependencies.contains_key(local_id) {
            usage_map.entry(local_id.clone()).or_default().push(seg_idx);
        }
    }
}
```

**Step 3 —** `build_main_module_usage_set` (`dependency_analysis.rs`, line 341):

Returns `HashSet<Id>` of root variables still referenced from "active" module items — any item that is NOT a `Decl`, `Import`, `ExportNamed`, or `ExportAll`. These are items that execute at module load time (e.g., expression statements, default exports).

```rust
let should_check = !matches!(item,
    Stmt::Decl(_) | Import(_) | ExportNamed(_) | ExportAll(_)
);
```

* * *

#### Step 4: `find_migratable_vars`

**Source:** `dependency_analysis.rs`, `find_migratable_vars` function (lines 382–475).

Returns `BTreeMap<usize, Vec<Id>>` — segment index → list of var ids to migrate.

**Four conditions for migration (all must hold):**

| Condition | Check |
| --- | --- |
| Exactly one segment uses it | `segments_using.len() == 1` |
| Not referenced by root module runtime code | `!main_module_usage.contains(id)` |
| Not user-exported | `!dep_info.is_exported` |
| Not itself an import | `!dep_info.is_imported` |

**Transitive dependency collection:** For each candidate, `collect_transitive_dependencies` is called (BFS walk). If variable `A` depends on variable `B` and `B` also satisfies the four conditions, `B` is included in the migration set alongside `A`.

**Safety fixpoint loop (**`changed` **loop, lines 420–472):**

The initial candidate set may be unsound because a candidate might still be depended on by a root declaration that stays in the root module. The fixpoint loop iteratively removes unsafe candidates until no more removals are possible:

```plaintext
loop until changed == false:
    Build assignment: BTreeMap<Id, usize>  (candidate → target segment)
    For each candidate in each segment's list:
        Remove if main_module_usage still contains it
        Remove if shared_declarator_migrates_as_a_unit() returns false
            (i.e., a destructuring pattern like `const { a, b } = foo`
             where not ALL idents migrate to the same segment)
        Remove if used_by_incompatible_root == true:
            (a non-imported root declaration depends on the candidate
             AND that declaration is NOT migrating to the same segment)
    If any removal happened: changed = true, continue loop
    Else: break
```

The `BTreeMap` (not `HashMap`) is used for the `migratable` map to ensure **deterministic iteration order** — the comment in the source explicitly notes this.

* * *

#### Step 5: `precompute_and_declare_auto_exports`

**Source:** `parse.rs`, `precompute_and_declare_auto_exports` (lines 1354–1416).

Before any actual migration happens, this step pre-declares all `_auto_` exports that segment modules will need to import after migration. Without this step, a segment module's `new_module` call (Step 6) would try to import a root variable that hasn't been exported yet.

**Algorithm:** For each variable being migrated, examine its `depends_on` list. For each dependency that:

*   Is in `global_collect.root` (a root-level symbol)
    
*   Is NOT being migrated to the same segment
    
*   Is NOT already imported or exported
    

→ Insert `export { id as _auto_<sym> }` into `module.body` and register it in `global_collect`.

* * *

#### Steps 6–7: Populate and Strip

**Step 6 — Populate** `segment.data.migrated_root_vars` (`parse.rs`, `apply_variable_migration` lines 1010–1098):

For each `(seg_idx, var_ids)` in `migratable`:

1.  **Topological sort** (`sort_migrated_vars_topologically`): dependencies declared before dependents. Uses stable tie-breaking for determinism.
    
2.  **Cyclic variable detection** (`find_cyclic_migrated_vars`): if a variable's initializer references itself (direct recursion), it is a "cyclic var". Cyclic `Var` declarators are split into `let x; x = init` form via `create_cyclic_var_items` — this avoids the temporal dead zone.
    
3.  **Deduplication** (`seen_var_decls` HashSet): multiple `Id`s can point to the same `VarDeclarator` (destructuring patterns like `const [a, b] = foo`). The key is a `format!("{:?}", decl.name, decl.init)` string — each unique declarator is emitted only once.
    
4.  Build `ast::ModuleItem` for each var (Var/Fn/Class/TsEnum decl) → store in `unique_module_items`.
    
5.  Set `segments[seg_idx].data.migrated_root_vars = unique_module_items`.
    

**Step 7 — Strip migrated vars from segment idents** (`parse.rs`, lines 1091–1098):

```rust
segments[seg_idx].data.local_idents.retain(|id| !sorted_var_ids.contains(id));
segments[seg_idx].data.scoped_idents.retain(|id| !sorted_var_ids.contains(id));
```

\*\*Pitfall 2 — Strip from \`local\_idents\`/\`scoped\_idents\`:\*\* After populating \`migrated\_root\_vars\`, the migrated ids MUST be removed from \`local\_idents\` and \`scoped\_idents\`. If they remain, \`new\_module\` will generate an \`import\` statement for them (Step 7 of \`new\_module\`'s import generation), producing a module with both \`import { x }\` and \`const x = ...\` for the same symbol — a duplicate binding error. The \`.retain()\` calls above are the canonical fix.

* * *

#### Steps 8–10: Root Module Cleanup

**Step 8 —** `remove_migrated_exports` (`parse.rs`, lines 1429–1474):

Removes from the root module:

*   `ExportNamed` specifiers that reference migrated ids. The `export {}` statement itself is removed if all its specifiers were for migrated ids.
    
*   `Stmt::Decl(Var)` declarators whose declared idents are fully migrated.
    
*   `Stmt::Decl(Fn)` / `Stmt::Decl(Class)` / `Stmt::Decl(TsEnum)` items for migrated ids.
    

Uses **full** `Id` **matching** (`(sym, SyntaxContext)` pair — not just symbol name) to avoid incorrectly removing a same-named variable from a different scope.

**Step 9 —** `remove_unused_qrl_declarations` (`parse.rs`, lines 1481–~1625):

After migration, some `const _qrl_*` and `const i_*` declarations in the root module may become orphaned — no remaining code references them. SWC's standard DCE cannot remove them because `may_have_side_effects` returns `true` for all `Call` expressions. This function does a targeted cleanup:

1.  Collect all idents **defined** by `_qrl_*`/`i_*` const declarations and by import declarations.
    
2.  Collect all idents **referenced** by non-removable items (everything except `_qrl_*`/`i_*` consts and imports).
    
3.  Propagate references transitively: if a used `_qrl_*` const references an import, that import is also kept.
    
4.  Remove any `_qrl_*`/`i_*` const declaration whose defined ident is not in the used set.
    
5.  Remove any import declaration that imports only symbols no longer referenced.
    
6.  **Loop** until stable (removing a `_qrl_*` const may make its referenced imports unused).
    

**Step 10 — Optional re-DCE** (`parse.rs`, ~line 419–428):

```rust
if config.minify != MinifyMode::None {
    program.mutate(&mut simplify::simplifier(unresolved_mark, simplify::Config {
        dce: simplify::dce::Config { preserve_imports_with_side_effects: false, .. },
    }));
}
```

Runs a final `simplify` pass to remove any import declarations that became unused after variable migration. Only runs if `minify != None`.

* * *

### Variable Migration — Concrete Before/After Example

**Root module before migration:**

```typescript
// src/routes/index.tsx (after QwikTransform, before migration)
import { component$, useSignal } from "@qwik.dev/core";

// Root variable — only used by the onClick segment
const THRESHOLD = 100;

export const Index = component$(() => {
  const count = useSignal(0);
  return <button onClick$={(e) => {
    if (count.value > THRESHOLD) { count.value = 0; }
  }}>click</button>;
});

// Auto-export injected by ensure_export for count (scoped capture)
export { count as _auto_count };

// Segment declaration emitted by QwikTransform
const _qrl_onClick = qrl(() => import('./index_onClick_abc123'), 'onClick_abc123');
```

**After** `apply_variable_migration`**:**

`THRESHOLD` qualifies for migration: used by only one segment (`onClick`), not user-exported, not an import, not referenced by root module runtime code.

```typescript
// Root module after migration
import { component$, useSignal } from "@qwik.dev/core";

export const Index = component$(() => {
  const count = useSignal(0);
  return <button onClick$={...}>click</button>;
});

export { count as _auto_count };

// THRESHOLD declaration removed — migrated to segment module
// _qrl_onClick may also be removed by remove_unused_qrl_declarations
```

**Generated segment module (**`index_onClick_abc123.ts`**) after** `new_module`**:**

```typescript
import { _captures } from "@qwik.dev/core";
import { _auto_count as count } from "./index";  // via ensure_export

// Migrated root variable — injected from migrated_root_vars
const THRESHOLD = 100;

export const onClick_abc123 = (e) => {
  const [count] = _captures;  // scoped capture destructuring
  if (count.value > THRESHOLD) { count.value = 0; }
};
```

Source: `apply_variable_migration`, `precompute_and_declare_auto_exports`, `remove_migrated_exports`, `remove_unused_qrl_declarations` in `parse.rs` (lines 979–1625); `analyze_root_dependencies`, `build_root_var_usage_map`, `build_main_module_usage_set`, `find_migratable_vars` in `dependency_analysis.rs` (lines 28–475).

* * *

## Chapter 5 Part 2: Segment Module Construction and Manifest Output

This chapter covers how the optimizer assembles each extracted segment closure into a standalone module file (`new_module`), the `ensure_export`/auto-export pipeline, the `inlinedQrl` preprocessing path (`handle_inlined_qsegment`), and the final manifest and output structs.

* * *

### Segment Module Construction: new\_module

`new_module` takes a `NewModuleCtx` and returns `(ast::Module, SingleThreadedComments)`. It is called once per segment in `transform_code` after all post-transform passes and variable migration have completed.

#### NewModuleCtx Fields

| Field | Type | Purpose |
| --- | --- | --- |
| `expr` | `Box<ast::Expr>` | The extracted closure expression |
| `path` | `&PathData` | Source file path data (file\_stem, file\_name, rel\_dir) |
| `name` | `&str` | Segment symbol name — used for the final named export |
| `local_idents` | `&[Id]` | Static import sources (vars the closure references from outer scope) |
| `scoped_idents` | `&[Id]` | Runtime captures — drives `_captures` injection |
| `global` | `&GlobalCollect` | Import/export metadata from parent module |
| `core_module` | `&Atom` | `"@qwik.dev/core"` or custom override |
| `need_transform` | `bool` | Whether to inject `_captures` destructuring into function body |
| `explicit_extensions` | `bool` | Whether to include file extensions in generated imports |
| `leading_comments` | `SingleThreadedCommentsMap` | Leading comment map (from parent file) |
| `trailing_comments` | `SingleThreadedCommentsMap` | Trailing comment map (from parent file) |
| `extra_top_items` | `&BTreeMap<Id, ast::ModuleItem>` | Shared hoisted QRL declarations from parent `QwikTransform` |
| `migrated_root_vars` | `&[ast::ModuleItem]` | Root module vars migrated to this segment by variable migration |
| `explicit_imports` | `&IndexMap<Id, Import>` | Dev-mode QRL helper imports (`qrlDev`, `inlinedQrlDev`, `noopQrlDev`) |

Source: `code_move.rs` `NewModuleCtx` struct (lines 105–120).

#### Step 1: \_captures Import Injection

**Gate:** `ctx.need_transform == true && !ctx.scoped_idents.is_empty()`.

When true: generate a private ident for `_captures`, push `import { _captures } from "<core_module>"` as the first module item via `create_synthetic_named_import`.

When false: `_captures = None`, no import generated.

The `_captures` value (an `Option<Id>`) is threaded into Steps 2–3.

Source: `code_move.rs` `new_module` (lines 134–143).

#### Step 2: transform\_function\_expr

If `_captures` is `Some`, the extracted expression is passed through `transform_function_expr(*ctx.expr, &_captures, ctx.scoped_idents)`.

**Dispatch:**

*   `Expr::Arrow` → `transform_arrow_fn`
    
*   `Expr::Fn` → `transform_fn`
    
*   Any other expression → pass through unchanged
    

Both `transform_arrow_fn` and `transform_fn` prepend `read_captures(_captures, scoped_idents)` as the first statement in the function body.

`read_captures` **output — individual const statements (NOT array destructuring):**

`read_captures` generates one `const` declarator per scoped\_ident using computed member access, not array destructuring syntax:

```typescript
// read_captures output for scoped_idents = [count, signal]
const count = _captures[0];
const signal = _captures[1];
```

The generated `VarDecl` has `kind: Const` and each declarator's init is `_captures[index]` via `MemberProp::Computed`.

**Arrow concise body conversion:** If the arrow expression has an `Expr` body (concise/expression body), it is converted to a block body:

1.  `read_captures` statement is prepended (only if `scoped_idents` is non-empty)
    
2.  A `return <expr>` statement is inserted after captures
    

Source: `code_move.rs` `transform_function_expr` (line 1113), `transform_arrow_fn` (line 1123), `transform_fn` (line 1160), `read_captures` (line 1194).

#### Steps 3–4: QRL Hoisting and Self-Referential Fix

**Step 3 —** `hoist_qrls_from_expr`**:** Walks the (possibly transformed) expression and extracts any nested `qrl()` or `inlinedQrl()` calls into module-level `var` declarations using a `QrlHoistingVisitor`. The nested call is replaced with a reference to the hoisted variable. For `inlinedQrl()` calls where the first argument is not an ident or null, a `_inlined_<name>` variable is created and hoisted. The returned `BTreeMap<Id, ast::ModuleItem>` contains the hoisted declarations; `var` (not `const`) is used so forward references are safe.

**Step 4 —** `fix_self_referential_vars_in_function`**:** Dispatches on expression type (Arrow → `fix_arrow_body_self_refs`, Fn → `fix_fn_body_self_refs`). Scans all `const x = ...` declarations where the initializer references `x` itself (detected by collecting idents from the init and comparing against the binding name). For single-declarator self-referential `const` blocks, the transformation emits:

1.  `const _ref = {}` (shared temp object)
    
2.  `_ref.x = <init using _ref.x>` (assignment using property access)
    
3.  `const {x} = _ref` (destructure back)
    

Source: `code_move.rs` `hoist_qrls_from_expr` (line 1225), `fix_self_referential_vars_in_function` (line 793).

#### Steps 5–6: Extra Top Items and Combined Local Idents

**Step 5 —** `collect_needed_extra_top_items` **(first call, for ident analysis):**

Walks `ctx.extra_top_items` (the BTreeMap of hoisted QRL declarations from parent `QwikTransform.extra_top_items`) to find which ones are transitively needed. The seed set is:

```plaintext
local_idents ∪ scoped_idents ∪ migrated_root_vars idents ∪ expr idents ∪ hoisted_qrl idents
```

Expands transitively via a fixpoint loop: if any item in `extra_top_items` is keyed by an Id (or sym) in the needed set, that item's defined idents are added to needed, triggering further expansion. The loop terminates when `needed` and `needed_syms` stop growing.

**Step 6 — Combined local idents expansion:**

Starts with `ctx.local_idents.to_vec()` as `combined_local_idents`. Adds any idents defined in `hoisted_qrls` and `extra_top_items` that are not already in `local_idents_set`. The expanded set drives import generation in Step 7.

Source: `code_move.rs` `new_module` (lines 165–195), `collect_needed_extra_top_items` (line 453).

\*\*OXC Pitfall 4 — Double call of \`collect\_needed\_extra\_top\_items\`:\*\* The function is called twice in \`new\_module\` — once before import generation (Step 5, for ident analysis) and once after import generation (Step 8, for actual item insertion). This is a known implementation redundancy. Both calls use the same arguments but the second call runs after collision renaming, so the results may differ if hoisted ident names changed. An OXC implementor must call the equivalent function twice at the same locations.

#### Step 7: Import Generation (Two-Pass with Collision Detection)

For each ident in `combined_local_idents`, the optimizer resolves an import source using a priority cascade:

| Priority | Source | Condition |
| --- | --- | --- |
| 1 | `ctx.global.imports` exact `(sym, ctxt)` match | `global_imports.get(id)` returns `Some` |
| 2 | `ctx.explicit_imports` exact match | `explicit_imports.get(id)` returns `Some` |
| 3 | Unique-sym fallback | Single match by sym only across `global_imports` (then `explicit_imports`) |
| 4 | Parent module export | `ctx.global.has_export_symbol(&id.0)` is true — generates `import { name } from './file_stem'` |

Parent module imports (Priority 4) are emitted directly into `module.body` during the first pass. Priorities 1–3 are collected into `seen_import_names: HashMap<Atom, Vec<(Id, Import)>>` for collision detection.

**First pass:** For each ident in `combined_local_idents`, call `resolve_import_for_id` (Priority 1–3). If found, push `(id, import)` into `seen_import_names[id.sym]`. If not found but ident is exported by parent, emit import directly.

**Second pass (sorted by name for stability):** Iterate `seen_import_names` sorted alphabetically:

*   `len == 1`: emit directly via `build_import_decl(import, id)`
    
*   `len > 1` (collision): first entry keeps the original name; subsequent entries get `_name_1`, `_name_2` suffix (index-based, 1-indexed for collisions after the first)
    

**Concrete collision example:**

```typescript
// Two different sources both export a symbol named "handler"
// Priority-1 finds handler from "@lib/a"
// Priority-1 also finds handler from "@lib/b"

// Generated imports after collision resolution:
import { handler } from "@lib/a";       // index 0 — keeps original
import { handler as handler_1 } from "@lib/b";  // index 1 — renamed
```

Source: `code_move.rs` `resolve_import_for_id` (line 66), `new_module` import generation loop (lines 197–276).

#### Steps 8–12: Deduplication, Separation, and Topological Sort

**Step 8 —** `collect_needed_extra_top_items` **(second call):** Called again with the same arguments to get the filtered `extra_top_items` list for insertion. This is the redundant second call documented in Pitfall 4.

**Step 9 — Deduplication against already-defined symbols:** Collects `already_imported_syms` from all `ImportDecl` items currently in `module.body` (the imports added in Steps 1 and 7). Extends into `already_defined_syms` with idents from `hoisted_qrls`, `migrated_root_vars`, and `ctx.global.imports` keys. Filters `extra_top_items` to exclude any item whose defined symbols overlap `already_defined_syms`.

**Step 10 — Separate extra\_top\_items into imports and non-imports:** Partitions the deduplicated `extra_top_items` using `matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_)))`. Import items are emitted immediately (appended to `module.body`). Non-import items enter the topological sort pool.

**Step 11 —** `order_items_by_dependency` **topological sort:** Receives `combined_items = hoisted_qrls.into_values() + migrated_root_vars + extra_non_imports`. Builds a dependency graph using symbol names only (ignoring `SyntaxContext`): each item's defined symbols are mapped to its index; each item's used symbols (minus its own definitions) create dependency edges to the items that define them. Uses Kahn's algorithm (BFS topological sort) with stable ordering (`ready` sorted before each iteration).

**Cycle handling:** If the ordered result is shorter than the input (cyclic items remain), cyclic items are appended at the end. If a cyclic item is a single-declarator `const` with a QRL pattern in its init, it is split into `let x;` + `x = <init>` to break the temporal dead zone.

**Step 12 — Aggressive final deduplication:** Scans the entire assembled `module.body` (all items from Steps 1, 7, 10, and topological sort). Any item whose defined symbol name (sym as `Atom`) was already seen in `final_seen_syms` is dropped. Covers imports, `const` declarations, and export declarations. Uses sym-only comparison (ignores `SyntaxContext`).

Source: `code_move.rs` `new_module` (lines 278–447), `order_items_by_dependency` (line 662).

#### Step 13: create\_named\_export

The last item pushed to `module.body` is always:

```typescript
export const <name> = <expr>;
```

`create_named_export(expr, ctx.name)` builds an `ExportDecl` wrapping a `VarDecl` with `kind: Const`. The binding identifier is `Atom::from(ctx.name)`. This is the segment's sole public export.

Source: `code_move.rs` `create_named_export` (line 1091).

#### Complete Segment Module Structure Example

Given `component$((props) => { ... })` with one scoped capture `count` and one migrated root var `THRESHOLD`:

```typescript
// Step 1: _captures import
import { _captures } from "@qwik.dev/core";

// Step 7 (priority 4): parent module import for local_idents
import { useSignal } from "@qwik.dev/core";

// Step 7 (priority 4): auto-exported import from parent
import { _auto_count as count } from "./index";

// Step 11: migrated root var (topologically sorted)
const THRESHOLD = 100;

// Step 13: named export
export const component_onClick_abc123 = (props) => {
  // Step 2: read_captures output — individual const per scoped_ident
  const count = _captures[0];
  if (count.value > THRESHOLD) { count.value = 0; }
};
```

* * *

### ensure\_export and Auto-Export Injection

`ensure_export(id)` is called during `handle_inlined_qsegment` and `create_segment` whenever a segment's `local_idents` include an ident that is a root-level symbol in the parent module (not already exported as a named export).

**Algorithm:**

1.  Get `canonical_id = global_collect.canonical_id_for(id)` (resolves to the defining declaration's Id)
    
2.  Compute exported name: `_auto_<canonical_id.sym>` (e.g., `count` → `_auto_count`)
    
3.  Call `global_collect.add_export(canonical_id.clone(), Some(exported_name.clone()))`
    
4.  If `add_export` returns `true` (newly added, not already present): insert `export { canonical_id as _auto_sym }` into `self.extra_bottom_items`
    

Source: `transform.rs` `ensure_export` (line 1271).

`extra_bottom_items` **in** `fold_module`**:** After all regular module items are processed by `fold_module`, `extra_bottom_items` are appended to the end of `module.body`. This is the opposite of `extra_top_items`, which are inserted at the beginning of segment modules.

**Segment module import resolution:** In `new_module` Step 7 (Priority 4), if an ident matches `global.has_export_symbol(&id.0)`, `resolve_export_for_id` returns the renamed export name. The generated import becomes:

```typescript
// If ensure_export added: export { count as _auto_count }
// Then in the segment module:
import { _auto_count as count } from "./index";
// or with explicit_extensions: from "./index.ts"
```

\*\*OXC Pitfall 3 — \`extra\_bottom\_items\` vs \`extra\_top\_items\` destination confusion:\*\* \`extra\_bottom\_items\` (auto-export injections from \`ensure\_export\`) are appended to the \*\*parent/root module\*\* by \`fold\_module\`. \`extra\_top\_items\` (shared hoisted QRL declarations from \`QwikTransform\`) are inserted at the \*\*top of segment modules\*\* by \`new\_module\`. They go to opposite locations. An OXC implementor must not confuse these two maps.

* * *

### Hoist Strategy: Inline Segment Emission (create\_inline\_qrl)

`create_inline_qrl` handles the non-standard case where a segment expression is used directly at the call site rather than emitted as a separate file.

**Gate:** `should_inline = EntryStrategy::Inline || EmitMode::Lib || expr is Ident`

**When** `should_inline == true`**:** The expression is used directly as the first argument to `inlinedQrl(expr, name[, captures])`. No separate segment file is pushed.

**When** `should_inline == false` **(Hoist with non-ident expr):**

1.  A private ident `<symbol_name>` is created
    
2.  A `qrl_id: Some(("q_<symbol_name>", empty_ctxt))` is set
    
3.  A new `Segment { entry: None, ... }` is pushed to `self.segments` with `expr: Box::new(expr)` and the private ident used in its place
    
4.  The segment will be emitted as an in-module `const <name> = <expr>` declaration by `fold_module` (Hoist step), not as a separate file
    

**In** `fold_module` **(Hoist drain):** Pending Hoist segments with `entry: None` are drained as top-level `const <name> = <expr>` declarations. If `qrl_id` is `Some`, an optional `q_name.s(value)` side-effect call may be emitted.

The call site receives `inlinedQrl(<ident>, name[, captures])` where `<ident>` is the private ident referencing the hoisted const.

Source: `transform.rs` `create_inline_qrl` (line 1945).

* * *

### inlinedQrl Preprocessing (handle\_inlined\_qsegment)

`handle_inlined_qsegment` is called by `fold_call_expr` when it encounters an existing `inlinedQrl()` call in source code (e.g., from compiled library code re-processed by the optimizer).

**Early exits:**

*   `EmitMode::Lib` → pass through unchanged (library re-emit, no reprocessing)
    
*   First argument is `null` literal → pass through unchanged
    

**Processing path:**

1.  **Reverse args:** `node.args.reverse()` (then pop args in original order — pop = first arg)
    
2.  **Extract symbol name:** Pop second arg (must be string literal); call `parse_symbol_name` to split into `(symbol_name, display_name, hash)`. Mode `Dev/Test/Hmr` includes display name suffix
    
3.  **Push/pop segment\_stack** around folding first arg: `self.segment_stack.push(symbol_name.clone()); let folded = first_arg.expr.fold_with(self); self.segment_stack.pop()`
    
4.  **Const-initializer inlining gate** (`!self.is_inline()`): If the folded expression is a simple ident referencing a non-exported local const, inline its initializer directly. Skip for `Inline/Hoist` strategies (the `.s()` call will use the ident directly). Skip for exported idents (segment can import from parent)
    
5.  **Extract scoped\_idents:** If third arg exists and is an array of idents, use those as `scoped_idents` (`Captures::Auto` or `Captures::Explicit`). Otherwise compute via `compute_scoped_idents`
    
6.  **ensure\_export for local\_idents:** For each ident in `local_idents` that matches a root symbol, call `self.ensure_export(&root_id)`
    
7.  **Route to** `create_inline_qrl` **or** `create_segment`**:** If `self.is_inline()`: call `create_inline_qrl`. Otherwise (Hoist/Smart/Single): call `create_segment`
    

Source: `transform.rs` `handle_inlined_qsegment` (line 498).

* * *

### Manifest and Output Format

The optimizer's final output is a `TransformOutput` containing per-file `TransformModule` records. For multi-file processing, `get_manifest()` assembles a `QwikManifest` from the segment modules.

**Scope boundary:** This chapter specifies the optimizer manifest only. The Qwik Vite plugin post-processes this into the final router manifest (adding route metadata, prefetch graph, etc.), which is outside this spec's scope.

* * *

### SegmentAnalysis

`SegmentAnalysis` is the per-segment metadata record included in each segment `TransformModule` and collected into `QwikManifest.symbols`.

| Field | Type | Semantics |
| --- | --- | --- |
| `origin` | `Atom` | Relative path of the source file (e.g., `"src/routes/index.tsx"`) |
| `name` | `Atom` | Full symbol name including hash (e.g., `"component_onClick_abc123"`) |
| `entry` | `Option<Atom>` | Bundler entry group key. `None` **\= inline/hoist/noop** (no separate file); `Some(key)` = separate file with this entry key |
| `display_name` | `Atom` | Human-readable name without hash (e.g., `"component_onClick"`) |
| `hash` | `Atom` | 8-char base64 hash string |
| `canonical_filename` | `Atom` | Filename stem without extension (e.g., `"component_onClick_abc123"`) |
| `path` | `Atom` | Relative directory path (e.g., `"src/routes"`) |
| `extension` | `Atom` | Output extension (e.g., `"js"`, `"ts"`) — computed from `transpile_ts/jsx` flags |
| `parent` | `Option<Atom>` | Parent segment name if nested inside another segment; `None` for top-level segments |
| `ctx_kind` | `SegmentKind` | Enum: `Function` or `JSXProp` — determined by `ctx_name` prefix `"on"` |
| `ctx_name` | `Atom` | Name of the enclosing marker function (e.g., `"component$"`, `"onClick$"`) |
| `captures` | `bool` | `!scoped_idents.is_empty()` — whether the segment has runtime captures |
| `loc` | `(u32, u32)` | Byte offsets `(span.lo, span.hi)` in the source file |
| `param_names` | `Option<Vec<Atom>>` | Parameter names of the closure (skipped in JSON if `None`) |
| `capture_names` | `Option<Vec<Atom>>` | Names of captured variables — \`Some(scoped\_idents.map( |

`entry` **field semantics:** The `entry` field is the most counterintuitive field in `SegmentAnalysis`.

*   `entry: None` — the segment does **not** get a separate output file. This applies to:
    
    *   `EntryStrategy::Inline` segments (inlined directly at call site)
        
    *   `EntryStrategy::Hoist` segments with non-ident expr (hoisted to module-level const)
        
    *   Noop QRL segments (`create_noop_qrl` with `entry: None` on routeloader-split)
        
*   `entry: Some(key)` — the segment is a bundler entry point. The `key` groups multiple segments that should be bundled together (e.g., `"a_chunk"`)
    

Source: `parse.rs` `SegmentAnalysis` struct (lines 47–67).

* * *

### TransformModule

`TransformModule` is one record per output file in `TransformOutput.modules`.

| Field | Type | Semantics |
| --- | --- | --- |
| `path` | `String` | Output file path (relative) |
| `code` | `String` | Generated JavaScript/TypeScript source code |
| `map` | `Option<String>` | Source map JSON (if `source_maps: true` in options) |
| `segment` | `Option<SegmentAnalysis>` | Present for segment modules; `None` for the root module |
| `is_entry` | `bool` | `true` if this module is a bundler entry point |
| `order` | `u64` | Sort key (not serialized) — used for stable output ordering |

`is_entry` **inversion:**

```rust
let is_entry = h.entry.is_none();
```

\*\*OXC Pitfall 1 — \`is\_entry\` inverts expected semantics:\*\* \`is\_entry = true\` means \`entry: None\` — i.e., the segment is an inline/hoist segment that is \*\*not\*\* a separate bundler entry. This is the opposite of the intuitive reading. Segments with \`entry: Some(key)\` have \`is\_entry = false\`. The field name reflects "is this an inlined entry (not a separate chunk)?" rather than "is this a bundler entry point?". An OXC implementor must set \`is\_entry = segment.entry.is\_none()\`.

**Path construction for segment modules:**

```rust
let path_str = h.data.path.to_string();
let path = if path_str.is_empty() { path_str } else { [&path_str, "/"].concat() };
let segment_path = [path, [&h.canonical_filename, ".", &h.data.extension].concat()].concat();
```

Example: `path = "src/routes"`, `canonical_filename = "component_onClick_abc123"`, `extension = "js"` → `"src/routes/component_onClick_abc123.js"`.

**Path construction for root module:**

```rust
let a = if did_transform && !config.preserve_filenames {
    [&path_data.file_stem, ".", &extension].concat()  // e.g. "index.js"
} else {
    path_data.file_name  // original filename unchanged
};
let path = path_data.rel_dir.join(a).to_slash_lossy().to_string();
```

`order` **field:**

*   Segment modules: `order = h.hash` (the segment struct's `hash` field, set from `new_ident.ctxt.as_u32() as u64` for Hoist or from the full hash computation for normal segments)
    
*   Root module: `order = DefaultHasher::hash(path.as_bytes())`
    

`TransformOutput.modules` is sorted by `order` via `modules.sort_unstable_by_key(|key| key.order)` before the final output.

Source: `parse.rs` `TransformModule` struct (lines 181–194), construction (lines 448–616).

* * *

### QwikBundle

`QwikBundle` represents a single output bundle in the manifest. In optimizer output (before the bundler runs), each segment maps to exactly one bundle.

| Field | Type | Semantics |
| --- | --- | --- |
| `size` | `usize` | Byte length of the generated code string (`module.code.len()`) |
| `symbols` | `Vec<Atom>` | List of segment names contained in this bundle |

In optimizer output, `symbols` always contains exactly one name. The bundler subsequently merges multiple segments into shared bundles.

Source: `parse.rs` `QwikBundle` struct (lines 120–125).

* * *

### QwikManifest

`QwikManifest` is the top-level manifest assembling all segment metadata.

| Field | Type | Semantics |
| --- | --- | --- |
| `version` | `Atom` | Always `"1"` |
| `symbols` | `HashMap<Atom, SegmentAnalysis>` | Map from segment name to full analysis record |
| `bundles` | `HashMap<Atom, QwikBundle>` | Map from filename (with extension) to bundle |
| `mapping` | `HashMap<Atom, Atom>` | Map from segment name to filename |

`get_manifest()` **construction loop:**

```rust
for module in &self.modules {
    if let Some(segment) = &module.segment {
        let filename = format!("{}.{}", segment.canonical_filename, segment.extension);
        manifest.mapping.insert(segment.name.clone(), filename.clone());
        manifest.symbols.insert(segment.name.clone(), segment.clone());
        manifest.bundles.insert(filename, QwikBundle {
            symbols: vec![segment.name.clone()],
            size: module.code.len(),
        });
    }
}
```

Root modules (no `segment` field) are excluded from the manifest. Only segment modules with `segment: Some(_)` are included.

**Manifest JSON example:**

```json
{
  "version": "1",
  "symbols": {
    "component_onClick_abc123": {
      "origin": "src/routes/index.tsx",
      "name": "component_onClick_abc123",
      "entry": "a_chunk",
      "displayName": "component_onClick",
      "hash": "abc123de",
      "canonicalFilename": "component_onClick_abc123",
      "path": "src/routes",
      "extension": "js",
      "parent": null,
      "ctxKind": "function",
      "ctxName": "component$",
      "captures": true,
      "loc": [120, 185]
    }
  },
  "bundles": {
    "component_onClick_abc123.js": {
      "size": 312,
      "symbols": ["component_onClick_abc123"]
    }
  },
  "mapping": {
    "component_onClick_abc123": "component_onClick_abc123.js"
  }
}
```

Source: `parse.rs` `QwikManifest` struct (lines 127–134), `get_manifest()` (lines 149–178).

* * *

### TransformOutput

`TransformOutput` is the top-level return value of `transform_code`.

| Field | Type | Semantics |
| --- | --- | --- |
| `modules` | `Vec<TransformModule>` | All output modules, sorted by `order` |
| `diagnostics` | `Vec<Diagnostic>` | Errors and warnings emitted during transformation |
| `is_type_script` | `bool` | Whether the input was TypeScript |
| `is_jsx` | `bool` | Whether the input contained JSX |

**Sort:** `modules.sort_unstable_by_key(|key| key.order)` is applied before returning. Segment modules are ordered by their hash; the root module is ordered by `DefaultHasher(path bytes)`.

`append` **merge behavior:** For multi-file processing (e.g., `transform_modules`), multiple `TransformOutput` values are merged:

```rust
pub fn append(mut self, output: &mut Self) -> Self {
    self.modules.append(&mut output.modules);
    self.diagnostics.append(&mut output.diagnostics);
    self.is_type_script = self.is_type_script || output.is_type_script;
    self.is_jsx = self.is_jsx || output.is_jsx;
    self
}
```

The boolean flags use logical OR — once `is_type_script` or `is_jsx` is set to true for any file in the batch, the merged output reports `true` for that flag.

Source: `parse.rs` `TransformOutput` struct (lines 111–118), `append` (lines 140–147).

* * *

## Chapter 6: Diagnostics and Error Behavior

### Overview: Diagnostic Subsystem

In the current `build/v2` branch, the Qwik optimizer emits diagnostics through **one path**: the SWC HANDLER path for error codes C02, C03, and C05.

**SWC HANDLER path (error codes C02, C03, C05):** The three optimizer error codes are emitted via SWC's `HANDLER` thread-local. Each call to `struct_err_with_code` or `struct_span_err_with_code` enqueues the diagnostic into SWC's error buffer. After `fold_module` completes, `handle_error` in `parse.rs` drains that buffer and converts each SWC diagnostic into a `Diagnostic` struct with `category: DiagnosticCategory::Error`. These diagnostics surface in `TransformOutput.diagnostics` alongside the normal modules.

\*\*Planned addition — PR #8482:\*\* A second diagnostic path for \`each\_transform\` warnings is introduced in the \`v2-auto-each-optimization\` branch. That path bypasses SWC's HANDLER and pushes \`Diagnostic\` structs directly onto a new \`diagnostics: Vec\` field on \`QwikTransform\`. See the 'Each Transform Diagnostic Warnings' section below for full details.

* * *

### The `errors.rs` Error Enum

The `errors.rs` file (13 lines total) defines the `Error` enum with explicit integer discriminants and a `get_diagnostic_id` format function:

```rust
// Source: packages/optimizer/core/src/errors.rs (full file)
use swc_common::errors::DiagnosticId;

pub enum Error {
    FunctionReference = 2,
    CanNotCapture,          // = 3 (implicit, one past FunctionReference)
    MissingQrlImplementation = 5,
}

pub fn get_diagnostic_id(err: Error) -> DiagnosticId {
    let id = err as u32;
    DiagnosticId::Error(format!("C{:02}", id))
}
```

The `format!("C{:02}", id)` format string zero-pads to 2 digits, producing: `"C02"`, `"C03"`, `"C05"`. There is no C04 — the gap between discriminant 3 and discriminant 5 is intentional. No other error codes exist.

* * *

### Diagnostic Conversion: `handle_error`

After `fold_module` completes, `parse.rs` calls `handle_error` to drain SWC's error buffer and convert each SWC diagnostic into a `Diagnostic` struct. This is the only point at which SWC-path diagnostics become visible to the caller:

```rust
// Source: parse.rs lines 850-915 (simplified)
fn handle_error(error_buffer: &ErrorBuffer, origin: Atom, source_map: &Lrc<SourceMap>) -> Vec<Diagnostic> {
    // For each SWC diagnostic in the buffer:
    Diagnostic {
        file: origin.clone(),
        code,           // DiagnosticId::Error(s) → Some(s)  (e.g., Some("C02"))
                        // DiagnosticId::Lint(s)  → Some(s)
                        // No DiagnosticId       → None
        message,        // From diagnostic.message()
        highlights,     // From span_labels → Some(vec![SourceLocation]) or None (for span-less errors)
        suggestions,    // From diagnostic.suggestions
        category: DiagnosticCategory::Error,  // Hardcoded — ALWAYS Error for all SWC-path diagnostics
        scope: DiagnosticScope::Optimizer,    // Hardcoded — ALWAYS Optimizer
    }
}
```

Key facts for OXC implementors:

*   `category` is **hardcoded** to `DiagnosticCategory::Error`. There is no conditional — all C-code diagnostics emerge as errors regardless of how they were emitted internally.
    
*   `scope` is **hardcoded** to `DiagnosticScope::Optimizer`.
    
*   `code` comes from the `DiagnosticId::Error(s)` variant — the string `s` is exactly what `get_diagnostic_id` produced (`"C02"`, `"C03"`, `"C05"`), wrapped in `Some`.
    
*   `highlights` comes from `span_labels`. Span-less errors (C02 uses `struct_err_with_code`) produce `highlights: None` because there are no span labels to convert.
    

Source: `parse.rs` lines 850–915.

* * *

### C02: FunctionReference

**Trigger:** A `local_idents` entry (a module-level identifier referenced inside a `$` closure) is present in `invalid_decl` — the subset of `decl_stack` entries with `IdentType::Fn` or `IdentType::Class`. This means the captured identifier is a function declaration or class declaration, not a variable.

`invalid_decl` **partition:**

```rust
// Source: transform.rs lines 967-972
let (decl_collect, invalid_decl): (_, Vec<_>) = self
    .decl_stack
    .iter()
    .flat_map(|v| v.iter())
    .cloned()
    .partition(|(_, t)| matches!(t, IdentType::Var(_)));
```

`IdentType::Fn` is pushed by `fold_fn_decl` (line 3487); `IdentType::Class` is pushed by `fold_class_decl` (line 3794). Every entry that is NOT `IdentType::Var(_)` ends up in `invalid_decl`.

**Emission site:** Step 6 of `_create_synthetic_qsegment`, **non-Lib path only**, inside the `should_emit` block:

```rust
// Source: transform.rs lines 1022-1042
if should_emit {
    for id in &segment_data.local_idents {
        if !self.options.global_collect.has_export_symbol(&id.0) {
            if let Some(root_id) = self.options.global_collect.root_id_for_symbol(&id.0) {
                self.ensure_export(&root_id);
            }
            if invalid_decl.iter().any(|entry| entry.0 == *id) {
                HANDLER.with(|handler| {
                    handler
                        .struct_err_with_code(  // NOTE: no span argument
                            &format!(
                                "Reference to identifier '{}' can not be used inside a Qrl($) scope because it's a function",
                                id.0
                            ),
                            errors::get_diagnostic_id(errors::Error::FunctionReference),
                        )
                        .emit();
                });
            }
        }
    }
}
```

**Message format:**

```plaintext
Reference to identifier '{name}' can not be used inside a Qrl($) scope because it's a function
```

where `{name}` is the identifier string (e.g., `myFn`).

**Span:** None. `struct_err_with_code` (not `struct_span_err_with_code`) is used. The resulting `Diagnostic` has `highlights: None`.

**Scope:** Non-Lib path only (`should_emit` is only set to `true` in the non-Lib branch of `_create_synthetic_qsegment`). In `EmitMode::Lib`, this error is never emitted.

\*\*C02 is the ONLY diagnostic code without a source span.\*\* C03 and C05 both use \`struct\_span\_err\_with\_code\` which attaches a span. C02 uses \`struct\_err\_with\_code\`. OXC must emit this diagnostic without location information (\`highlights: None\`). Attempting to synthesize a span position for C02 would be incorrect.

Cross-reference: The `should_emit` block containing this emission is in Step 6 of `_create_synthetic_qsegment` — see Chapter 3 §`_create_synthetic_qsegment` Step 6.

Source: `transform.rs` lines 967–972 (partition), lines 1022–1042 (emission), lines 3487, 3794 (IdentType push sites).

* * *

### C03: CanNotCapture

**Trigger:** `!can_capture_scope(first_arg)` is true (the `$` argument is not a function expression or arrow expression) **AND** `!scoped_idents.is_empty()` is true **after** `compute_scoped_idents` has already run.

```rust
// Source: transform.rs lines 4696-4698
const fn can_capture_scope(expr: &ast::Expr) -> bool {
    matches!(expr, &ast::Expr::Fn(_) | &ast::Expr::Arrow(_))
}
```

Only `Expr::Fn` and `Expr::Arrow` are considered valid capture scopes. Any other expression kind — object literals, identifier references, call expressions, etc. — returns `false`.

**Emission sites:** C03 is emitted at two identical call sites in `_create_synthetic_qsegment`:

```rust
// Source: transform.rs lines 992-1003 (non-Lib path)
// Identical structure at lines 904-916 (Lib path)
if !can_capture && !scoped_idents.is_empty() {
    HANDLER.with(|handler| {
        let ids: Vec<_> = scoped_idents.iter().map(|id| id.0.as_ref()).collect();
        handler
            .struct_span_err_with_code(
                first_arg_span,
                &format!("Qrl($) scope is not a function, but it's capturing local identifiers: {}", ids.join(", ")),
                errors::get_diagnostic_id(errors::Error::CanNotCapture),
            )
            .emit();
    });
    scoped_idents = vec![];
}
```

**Message format:**

```plaintext
Qrl($) scope is not a function, but it's capturing local identifiers: {ids}
```

where `{ids}` is a comma-space-joined list of the captured identifier names (e.g., `count, setCount`).

**Span:** `first_arg_span` — the span of the first argument to the `$` call. Produces `highlights: Some([...])`.

**Post-emission:** After emitting C03, `scoped_idents` is set to `vec![]` (cleared). Downstream code sees an empty capture list.

\*\*C03 is emitted AFTER \`compute\_scoped\_idents\` runs, not before.\*\* A non-function expression with zero captures is valid and produces no error. OXC must run the full scope analysis first, then check \`!can\_capture && !scoped\_idents.is\_empty()\`. Pre-filtering non-function expressions before scope analysis would incorrectly silence C03 for cases with actual captures.

Cross-reference: This emission is in Step 4 of `_create_synthetic_qsegment` — see Chapter 3 §`_create_synthetic_qsegment` Step 4, `compute_scoped_idents` call.

Source: `transform.rs` lines 904–916 (Lib path), lines 985–1003 (non-Lib path), lines 4696–4698 (`can_capture_scope`).

* * *

### C05: MissingQrlImplementation

**Trigger:** A locally-defined `$`\-suffixed function is called (the callee identifier ends with `$` and is not found in `global_collect.imports`), but its Qrl-suffixed counterpart (`useMyHook$` → `useMyHookQrl`) is not present in `global_collect.exports`.

**Emission site:** `fold_call_expr`, inside the `marker_functions` branch, in the local `$`\-export sub-branch (the `else` branch when the identifier is NOT in `global_collect.imports`):

```rust
// Source: transform.rs lines 4053-4075
} else {
    // ident is NOT in global_collect.imports — it is a locally defined $-function
    let new_specifier = convert_qrl_word(&ident.sym).expect("Specifier ends with $");
    // convert_qrl_word: strips trailing '$', appends "Qrl"
    // e.g., "useMyHook$" → "useMyHookQrl"
    global_collect
        .exports
        .get(&new_specifier)
        .map_or_else(
            || {
                HANDLER.with(|handler| {
                    handler
                        .struct_span_err_with_code(
                            ident.span,
                            &format!("Found '{}' but did not find the corresponding '{}' exported in the same file. Please check that it is exported and spelled correctly", &ident.sym, &new_specifier),
                            errors::get_diagnostic_id(errors::Error::MissingQrlImplementation),
                        )
                        .emit();
                });
            },
            |export_info| {
                replace_callee = Some(new_ident_from_id(&export_info.local_id).as_callee());
            },
        );
}
```

`convert_qrl_word` **(lines 179–188):** Strips the trailing `$` character (`QRL_SUFFIX = '$'`) and appends `"Qrl"` (`LONG_SUFFIX = "Qrl"`). Example: `useMyHook$` → `useMyHookQrl`, `component$` → `componentQrl`.

**Message format:**

```plaintext
Found '{sym}' but did not find the corresponding '{qrl_sym}' exported in the same file. Please check that it is exported and spelled correctly
```

where `{sym}` is the original identifier (e.g., `useMyHook$`) and `{qrl_sym}` is the Qrl-suffixed form (e.g., `useMyHookQrl`).

**Span:** `ident.span` — the span of the `$`\-suffixed callee identifier. Produces `highlights: Some([...])`.

\*\*C05 only fires for LOCAL \`$\`-exports, never for imports.\*\* If the \`$\`-function is an import (found in \`global\_collect.imports\`), the transform replaces the callee with the Qrl-suffixed import — no error is emitted. C05 only fires when the identifier is \*\*locally defined\*\* (not imported) AND its Qrl-suffixed counterpart is missing from \`global\_collect.exports\`. OXC must not emit C05 for any \`$\`-suffixed call to an imported function.

Source: `transform.rs` lines 4048–4076 (full `fold_call_expr` local branch), lines 179–188 (`convert_qrl_word`).

* * *

### Diagnostic Summary Table (C02, C03, C05)

| Code | Name | Category | Span | SWC Method | Emission Site | Trigger |
| --- | --- | --- | --- | --- | --- | --- |
| `C02` | `FunctionReference` | `Error` | None | `struct_err_with_code` | Step 6 of `_create_synthetic_qsegment`, non-Lib path only | A `local_idents` entry is in `invalid_decl` (`IdentType::Fn` or `IdentType::Class`) |
| `C03` | `CanNotCapture` | `Error` | `first_arg_span` | `struct_span_err_with_code` | Step 4 of `_create_synthetic_qsegment`, both Lib and non-Lib paths | `!can_capture_scope(first_arg)` AND `!scoped_idents.is_empty()` after `compute_scoped_idents` |
| `C05` | `MissingQrlImplementation` | `Error` | `ident.span` | `struct_span_err_with_code` | `fold_call_expr`, `marker_functions` branch, local `$`\-export `else` sub-branch | Locally-defined `$`\-function called but Qrl-suffixed counterpart absent from `global_collect.exports` |

* * *

### Each Transform Diagnostic Warnings (Planned — PR #8482)

\*\*Status: Planned addition — not in current \`build/v2\`.\*\* This section documents the diagnostic system introduced by PR #8482 (\`v2-auto-each-optimization\` branch). The \`diagnostics\` field on \`QwikTransform\` does not exist in the current \`build/v2\`. This follows the same current/planned split documented in Chapter 4's EachTransform section.

#### Emission Path

The each\_transform warnings use a completely different emission path from C02/C03/C05. There is no SWC HANDLER involvement at all:

```rust
// Source: each_transform.rs (v2-auto-each-optimization branch), push_each_candidate_warning
self.diagnostics.push(Diagnostic {
    category: DiagnosticCategory::Warning,
    code: Some(MAP_TO_EACH_DIRECTIVE.to_string()),  // "map-to-each"
    file: self.options.path_data.rel_path.to_slash_lossy().to_string().into(),
    message: message.into(),
    highlights: Some(vec![SourceLocation::from(&self.options.cm, span)]),
    suggestions: Some(vec![suggestion.into()]),
    scope: DiagnosticScope::Optimizer,
});
```

Key differences from the SWC HANDLER path:

| Property | Value | Notes |
| --- | --- | --- |
| `category` | `DiagnosticCategory::Warning` | Never `Error` — not post-processed by `handle_error` |
| `code` | `Some("map-to-each")` | Rule name string — NOT a `"C0N"` format code |
| `highlights` | Always `Some([...])` | `SourceLocation::from(&self.options.cm, span)` — always has span |
| `suggestions` | Always `Some([...])` | Each warning variant carries a suggestion string |
| `scope` | `DiagnosticScope::Optimizer` | Same as C-code diagnostics |

Source: `each_transform.rs` (v2-auto-each-optimization branch), `push_each_candidate_warning`.

* * *

#### Warning Variants

The four `EachCandidateWarning` variants and their exact message and suggestion text:

| Variant | Message | Suggestion | Trigger Condition |
| --- | --- | --- | --- |
| `MissingKey` | "This .map() was not optimized to Each because the returned JSX node is missing a key." | "Add a stable key to the returned JSX node." | `remove_key_from_jsx_element` or `remove_key_from_transpiled_jsx_call` returns `None` |
| `UsesSecondParamForKey` | "This .map() was not optimized to Each because the key uses the callback's index parameter." | "Use a stable key derived from the item instead of the callback's index parameter." | `contains_any_ident(key_expr, second_param_ids)` is true |
| `CallDerivedKey` | "This .map() was not optimized to Each because the key is derived from a function call." | "Use a stable key expression without function calls." | `expr_contains_call(key_expr)` is true |
| `NotSingleJsxNode` | "This .map() was not optimized to Each because the callback does not return a single JSX node." | "Return a single JSX node from the .map() callback." | `is_non_single_jsx_like(return_expr, jsx_functions)` is true |

OXC must produce message and suggestion text byte-identical to the table above. These strings are user-visible and must not be paraphrased.

Cross-reference: Chapter 4 documents the EachTransform candidacy rules and the `.map()` → `<Each>` rewrite pipeline. Chapter 6 documents only the diagnostic output from that pipeline.

Source: `each_transform.rs` (v2-auto-each-optimization branch), `EachCandidateWarning` enum and `push_each_candidate_warning`.

* * *

#### Silent Abort: UnsafeSlice

The `EachSliceError` enum governs abort behavior during slice-level analysis. Not all slice errors produce warnings:

| `EachSliceError` Variant | `slice_error_to_warning` Result | Effect |
| --- | --- | --- |
| `UsesSecondParamForKey` | `Some(EachCandidateWarning::UsesSecondParamForKey)` | Warning emitted via `push_each_candidate_warning` |
| `CallDerivedKey` | `Some(EachCandidateWarning::CallDerivedKey)` | Warning emitted via `push_each_candidate_warning` |
| `UnsafeSlice` | `None` | Rewrite skipped, **no warning emitted** |

\*\*UnsafeSlice is a silent abort.\*\* When \`slice\_error\_to\_warning\` returns \`None\` for an \`UnsafeSlice\` error, the \`.map()\` → \`\` rewrite is skipped silently with no diagnostic emitted. OXC must skip the rewrite without emitting any warning. Do not emit a generic fallback warning for \`UnsafeSlice\`.

Source: `each_transform.rs` (v2-auto-each-optimization branch), `EachSliceError` enum, `slice_error_to_warning`.

* * *

#### Opt-Out Comment Directive

A developer can suppress the each\_transform rewrite and all associated warnings by placing an opt-out comment on the line immediately before the `.map()` call:

```js
// @qwik-disable-next-line map-to-each
const elements = items.map((item) => <Item key={item.id} />);
```

Block comment form is also accepted:

```js
/* @qwik-disable-next-line map-to-each */
const elements = items.map((item) => <Item key={item.id} />);
```

The two directive constants:

```rust
// Source: transform.rs (v2-auto-each-optimization branch) line 48
const QWIK_DISABLE_NEXT_LINE_DIRECTIVE: &str = "qwik-disable-next-line";

// Source: each_transform.rs (v2-auto-each-optimization branch) line 3
const MAP_TO_EACH_DIRECTIVE: &str = "map-to-each";
```

**Parsing mechanism** (`extend_optimizer_disabled_rules_from_comment_text`):

1.  Strip `*`, `/`, `{`, `}`, and whitespace characters from the raw comment text
    
2.  Strip the leading `@` character to normalize the prefix
    
3.  Match the `qwik-disable-next-line` prefix; if matched, extract the remainder
    
4.  Split the remainder by commas and spaces to produce individual rule names
    
5.  Add each rule name to the `rules` set for the next-line span
    

**Suppression check** (`has_disabled_optimizer_rule(span, "map-to-each")`): Returns `true` if the `rules` set for the span contains `"map-to-each"` **OR** contains `"all"`. The `"all"` wildcard suppresses ALL optimizer rules for the next line, not only `map-to-each`.

\*\*The opt-out comment is \`// @qwik-disable-next-line map-to-each\`\*\* — not \`/\* no each \*/\`. Earlier project notes reference \`/\* no each \*/\` but that is incorrect. The correct directive uses the \`@qwik-disable-next-line\` prefix, not a \`no-\` prefix. OXC must implement the \`@qwik-disable-next-line\` mechanism, not any \`no-\*\` variant.

Source: `transform.rs` (v2-auto-each-optimization branch) lines 48, 3613–3640 (`optimizer_comment_disables_rule`, `extend_optimizer_disabled_rules_from_comment_text`).

* * *

### Full Diagnostic Output Summary

All optimizer diagnostics across both subsystems:

| Code | Name | Category | Span | Emission Path | Suppressible | Status |
| --- | --- | --- | --- | --- | --- | --- |
| `C02` | `FunctionReference` | `Error` | None | SWC HANDLER → `handle_error` | No | Current |
| `C03` | `CanNotCapture` | `Error` | `first_arg_span` | SWC HANDLER → `handle_error` | No | Current |
| `C05` | `MissingQrlImplementation` | `Error` | `ident.span` | SWC HANDLER → `handle_error` | No | Current |
| `"map-to-each"` | `EachCandidateWarning` (4 variants) | `Warning` | call span | Direct `diagnostics.push` | Yes (`@qwik-disable-next-line map-to-each` or `all`) | Planned (PR #8482) |

**OXC implementation checklist for this chapter:**

*   \[ \] Implement `errors.rs` enum with exact discriminants (2, 3, 5) and `"C{:02}"` format string
    
*   \[ \] Emit C02 without a span (`highlights: None`)
    
*   \[ \] Emit C03 only after `compute_scoped_idents` confirms non-empty captures
    
*   \[ \] Emit C05 only for locally-defined `$`\-functions (not imports)
    
*   \[ \] Implement `push_each_candidate_warning` with `DiagnosticCategory::Warning` and `code: "map-to-each"`
    
*   \[ \] Implement silent `UnsafeSlice` abort (no diagnostic)
    
*   \[ \] Implement `@qwik-disable-next-line map-to-each` comment directive with `"all"` wildcard support