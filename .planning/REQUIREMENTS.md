# Requirements: Qwik Optimizer OXC Port

**Defined:** 2026-02-19
**Core Value:** Snapshot parity with the SWC optimizer across all 162 test cases

## v1 Requirements

Requirements for milestone v1.0 — Full Snapshot Parity. Each maps to roadmap phases.

### Naming

- [x] **NAME-01**: Segment names use original attribute names for event handlers (`div_onClick` not `div_q_e_click`)
- [x] **NAME-02**: Display names include all intermediate scope elements (no dropped context)
- [x] **NAME-03**: Parent field uses segment name with hash format (not display name string)

### Metadata

- [x] **META-01**: Segment metadata includes `paramNames` array
- [x] **META-02**: Path field handling matches SWC (`path` field populated, not `canonicalFilename` prefix)

### Bugs & Correctness

- [x] **BUG-01**: TypeScript type annotations stripped when `transpile_ts=true`
- [x] **BUG-02**: Component options object preserved in `componentQrl()` second argument
- [x] **BUG-03**: Captured variables match SWC (correct vars, correct order)
- [x] **BUG-04**: All segments extracted (no missing segment files)
- [x] **BUG-05**: `qwik_router_inline` complex transformation handled correctly
- [x] **BUG-06**: Source comments preserved in output

### Signal & Props Transforms

- [x] **SIG-01**: `_fnSignal` wrapper emitted for derived signals in JSX children
- [x] **SIG-02**: `_wrapProp(obj, "prop")` 2-argument form generated
- [x] **PROP-01**: Inline component props destructuring and `_restProps` transformed
- [x] **QRL-01**: QRL calls hoisted to variables (not inlined at usage site)

### JSX

- [x] **JSX-01**: JSX key generation matches SWC values
- [x] **JSX-02**: JSX immutability flags match SWC values

### Imports

- [x] **IMP-01**: Import statement ordering matches SWC
- [x] **IMP-02**: No missing or extra imports (correct import set per module)
- [x] **IMP-03**: Import specifiers from same module merged into single import
- [x] **IMP-04**: Relative import paths match SWC format

## v2 Requirements

Deferred to future milestones. Tracked but not in current roadmap.

### Diagnostics

- **DIAG-01**: Custom QWIK(x) error/warning messages emitted for invalid patterns
- **DIAG-02**: Source locations included in diagnostic output

### Performance

- **PERF-01**: Parallel file transformation via rayon
- **PERF-02**: Benchmark suite comparing OXC vs SWC optimizer throughput

### Transpilation

- **TRANS-01**: Full TS/JSX transpilation via OXC transformer (beyond type stripping)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Entry strategy variations beyond snapshot coverage | Match SWC behavior only; novel strategies are v2+ |
| Source map accuracy validation | Snapshot tests don't cover source maps; defer to integration testing |
| NAPI/Node.js bindings | Infrastructure concern, not optimizer correctness |
| Qwik v2 optimizer changes | This port targets current SWC behavior |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| NAME-01 | Phase 1 | Complete |
| NAME-02 | Phase 1 | Complete |
| NAME-03 | Phase 1 | Complete |
| META-01 | Phase 2 | Complete |
| META-02 | Phase 2 | Complete |
| BUG-01 | Phase 3 | Complete |
| BUG-02 | Phase 3 | Complete |
| BUG-03 | Phase 3 | Complete |
| BUG-04 | Phase 3 | Complete |
| BUG-05 | Phase 3 | Complete |
| BUG-06 | Phase 3 | Complete |
| SIG-01 | Phase 4 | Complete |
| SIG-02 | Phase 4 | Complete |
| PROP-01 | Phase 4 | Complete |
| QRL-01 | Phase 4 | Complete |
| JSX-01 | Phase 5 | Complete |
| JSX-02 | Phase 5 | Complete |
| IMP-01 | Phase 6 | Complete |
| IMP-02 | Phase 6 | Complete |
| IMP-03 | Phase 6 | Complete |
| IMP-04 | Phase 6 | Complete |

**Coverage:**
- v1 requirements: 21 total
- Mapped to phases: 21
- Unmapped: 0

---
*Requirements defined: 2026-02-19*
*Last updated: 2026-02-21 after Phase 8 completion*
