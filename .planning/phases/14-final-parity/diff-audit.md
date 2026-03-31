# Phase 14 Diff Audit (90 remaining diffs)

**Date:** 2026-02-24
**Baseline:** Post-Phase 13 + hash-before-oxfmt fix (72/162 exact matches)

## Per-file categorization

| Test | Categories |
|------|-----------|
| destructure_args_colon_props3 | SIGNAL_WRAP, CAPTURES, DCE |
| destructure_args_inline_cmp_block_stmt | IMPORT_ORDER, JSX_FLAGS, SPREAD_PROPS |
| destructure_args_inline_cmp_block_stmt2 | IMPORT_ORDER, JSX_FLAGS, SPREAD_PROPS |
| destructure_args_inline_cmp_expr_stmt | IMPORT_ORDER, JSX_FLAGS, SPREAD_PROPS |
| example_10 | OTHER, SHORTHAND |
| example_11 | ENTRY_FIELD |
| example_8 | DCE |
| example_component_with_event_listeners_inside_loop | SIGNAL_WRAP, HOIST, CAPTURES |
| example_default_export | ENTRY_FIELD |
| example_derived_signals_children | IMPORT_ORDER |
| example_derived_signals_cmp | IMPORT_ORDER, SHORTHAND |
| example_derived_signals_complext_children | IMPORT_ORDER, DCE |
| example_derived_signals_div | IMPORT_ORDER, SHORTHAND |
| example_derived_signals_multiple_children | IMPORT_ORDER |
| example_dev_mode_inlined | IMPORT_ORDER, DEV_MODE, JSX_FLAGS |
| example_dev_mode | DEV_MODE, JSX_FLAGS |
| example_drop_side_effects | IMPORT_ORDER, DEV_MODE, DCE, JSX_FLAGS |
| example_exports | LINE_WRAP, DCE, OTHER |
| example_functional_component_2 | SIGNAL_WRAP, JSX_FLAGS, OTHER |
| example_functional_component_capture_props | CAPTURES, JSX_FLAGS |
| example_functional_component | DCE |
| example_getter_generation | SHORTHAND, SIGNAL_WRAP |
| example_immutable_analysis | IMPORT_ORDER, LINE_WRAP, CTX_KIND, SIGNAL_WRAP, JSX_FLAGS, SPREAD_PROPS |
| example_inlined_entry_strategy | DCE, CAPTURES |
| example_input_bind | IMPORT_ORDER, SHORTHAND |
| example_invalid_references | LINE_WRAP, DCE |
| example_invalid_segment_expr1 | DCE, DIAGNOSTIC |
| example_issue_33443 | IMPORT_ORDER, SIGNAL_WRAP, CAPTURES, HOIST |
| example_issue_4438 | IMPORT_ORDER |
| example_jsx_import_source | JSX_IMPORT |
| example_jsx | IMPORT_ORDER |
| example_lightweight_functional | CAPTURES, LINE_WRAP, SPREAD_PROPS |
| example_manual_chunks | ENTRY_FIELD |
| example_missing_custom_inlined_functions | IMPORT_ORDER, DIAGNOSTIC |
| example_mutable_children | IMPORT_ORDER, LINE_WRAP, JSX_FLAGS |
| example_noop_dev_mode | DEV_MODE, JSX_FLAGS, HOIST |
| example_optimization_issue_3542 | CAPTURES, LINE_WRAP |
| example_optimization_issue_4386 | DCE |
| example_parsed_inlined_qrls | HOIST, JSX_IMPORT, SIGNAL_WRAP, OTHER |
| example_preserve_filenames_segments | FILE_EXT, JSX_FLAGS |
| example_preserve_filenames | FILE_EXT, IMPORT_ORDER, JSX_FLAGS |
| example_props_optimization | IMPORT_ORDER, SIGNAL_WRAP, CAPTURES, LINE_WRAP, SPREAD_PROPS |
| example_props_wrapping_children | IMPORT_ORDER, LINE_WRAP |
| example_props_wrapping_children2 | IMPORT_ORDER, LINE_WRAP |
| example_props_wrapping | IMPORT_ORDER, LINE_WRAP |
| example_props_wrapping2 | IMPORT_ORDER, LINE_WRAP |
| example_qwik_conflict | OTHER |
| example_qwik_react_inline | HOIST, LINE_WRAP, JSX_IMPORT, IMPORT_ORDER, DCE, OTHER |
| example_qwik_react | HOIST, IMPORT_ORDER, JSX_IMPORT, DCE, OTHER |
| example_qwik_router_inline | IMPORT_ORDER, LINE_WRAP, DCE, SHORTHAND |
| example_reg_ctx_name_segments_hoisted | IMPORT_ORDER, DCE, HOIST |
| example_reg_ctx_name_segments_inlined | IMPORT_ORDER, DCE, HOIST |
| example_reg_ctx_name_segments | IMPORT_ORDER, DCE, HOIST, JSX_FLAGS |
| example_server_auth | JSX_IMPORT, LINE_WRAP |
| example_spread_jsx | SPREAD_PROPS, LINE_WRAP, JSX_IMPORT |
| example_strip_client_code | IMPORT_ORDER, DCE, JSX_FLAGS, HOIST |
| example_strip_server_code | DCE |
| example_transpile_jsx_only | JSX_FLAGS |
| example_ts_enums_issue_1341 | OTHER, DCE |
| example_ts_enums_no_transpile | OTHER |
| example_ts_enums | OTHER, DCE |
| example_use_client_effect | LINE_WRAP |
| example_use_optimization | OTHER |
| example_use_server_mount | ENTRY_FIELD |
| issue_7216_add_test | SPREAD_PROPS, IMPORT_ORDER |
| relative_paths | HOIST, JSX_IMPORT, SIGNAL_WRAP, DCE |
| rename_builder_io | JSX_IMPORT, LINE_WRAP, IMPORT_ORDER |
| should_extract_single_qrl_2 | OTHER |
| should_extract_single_qrl_with_index | IMPORT_ORDER, LINE_WRAP |
| should_extract_single_qrl_with_nested_components | SHORTHAND, IMPORT_ORDER |
| should_extract_single_qrl | IMPORT_ORDER, LINE_WRAP |
| should_mark_props_as_var_props_for_inner_cmp | SIGNAL_WRAP, SPREAD_PROPS, IMPORT_ORDER |
| should_merge_attributes_with_spread_props_before_and_after | SPREAD_PROPS |
| should_merge_attributes_with_spread_props | SPREAD_PROPS |
| should_move_bind_value_to_var_props | SPREAD_PROPS |
| should_move_props_related_to_iteration_variables_to_var_props | LINE_WRAP, JSX_FLAGS |
| should_not_generate_conflicting_props_identifiers | IMPORT_ORDER, CAPTURES, HOIST |
| should_not_transform_events_on_non_elements | HOIST |
| should_not_wrap_var_template_string | LINE_WRAP, IMPORT_ORDER |
| should_split_spread_props_with_additional_prop | SPREAD_PROPS |
| should_split_spread_props_with_additional_prop5 | IMPORT_ORDER, JSX_FLAGS |
| should_transform_component_with_normal_function | SHORTHAND, IMPORT_ORDER |
| should_transform_multiple_event_handlers_case2 | SIGNAL_WRAP, IMPORT_ORDER |
| should_transform_multiple_event_handlers | SIGNAL_WRAP, IMPORT_ORDER |
| should_transform_nested_loops | IMPORT_ORDER, HOIST |
| should_transform_qrls_in_ternary_expression | OTHER |
| should_wrap_prop_from_destructured_array | SHORTHAND, IMPORT_ORDER, SIGNAL_WRAP, CAPTURES |
| should_wrap_store_expression | SIGNAL_WRAP, LINE_WRAP |
| special_jsx | OTHER |
| ternary_prop | IMPORT_ORDER |

## Category summary

| Category | Count | Nature |
|----------|-------|--------|
| IMPORT_ORDER | 44 | Cosmetic (accepted) |
| LINE_WRAP | 21 | Cosmetic (accepted) |
| DCE | 19 | Semantic |
| JSX_FLAGS | 17 | Semantic |
| HOIST | 14 | Semantic (architectural) |
| OTHER | 13 | Mixed |
| SPREAD_PROPS | 11 | Semantic |
| SIGNAL_WRAP | 11 | Semantic |
| CAPTURES | ~10 | Semantic |
| SHORTHAND | 8 | Cosmetic (OXC limitation) |
| JSX_IMPORT | 7 | Semantic |
| DEV_MODE | 4 | Semantic |
| ENTRY_FIELD | 4 | Semantic |
| DIAGNOSTIC | 2 | Semantic |
| FILE_EXT | 2 | Semantic |
| CTX_KIND | 1 | Semantic |

## Category definitions

- **IMPORT_ORDER** — Import statements in different order
- **LINE_WRAP** — Different line wrapping/formatting (not shorthand-related)
- **SHORTHAND** — OXC codegen `{x}` instead of `{x: x}` (unfixable OXC limitation)
- **DCE** — Dead code not eliminated or different dead code behavior
- **JSX_FLAGS** — Different immutability flag values in _jsxSorted/_jsxSplit
- **HOIST** — Different hoisting strategy (SWC hoists QRL callbacks to top-level named functions)
- **OTHER** — Various unique differences (TS enums, conflict renaming, comma operator, etc.)
- **SPREAD_PROPS** — Different spread props handling (_createElement vs _jsxSplit patterns)
- **SIGNAL_WRAP** — Missing or different _fnSignal/_wrapProp wrapping
- **CAPTURES** — Different capture variables or capture mechanism
- **JSX_IMPORT** — Different JSX import source or naming
- **DEV_MODE** — Dev mode QRL differences (file paths, _noopQrlDEV)
- **ENTRY_FIELD** — Different "entry" field in segment metadata
- **DIAGNOSTIC** — Different diagnostic messages/highlights
- **FILE_EXT** — Different output file extension
- **CTX_KIND** — Different ctxKind in metadata
