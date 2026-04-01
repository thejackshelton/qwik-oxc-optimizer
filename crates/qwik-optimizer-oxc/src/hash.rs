//! Segment hash computation, symbol escaping, and context naming.
//!
//! Provides:
//! - `compute_segment_hash` — 11-char SipHash-1-3 hash for a segment
//! - `escape_sym` — normalize arbitrary strings to valid identifier parts
//! - `register_context_name` — full 6-step naming pipeline
//! - `get_canonical_filename` — derive canonical_filename from display_name + symbol_name
//!
//! Algorithm (SPEC §Hash):
//!   1. Concatenate raw bytes (no separators): scope?, rel_path, display_name_core
//!   2. SipHash-1-3 with deterministic seed (0, 0)
//!   3. u64 → 8 little-endian bytes → base64url → replace `-` and `_` with `0`
//!
//! CRITICAL: Uses `siphasher::sip::SipHasher13` with seed (0, 0), NOT
//! `std::collections::hash_map::DefaultHasher` which is non-deterministic.

use std::collections::HashMap;
use std::hash::Hasher;

use base64::Engine;
use siphasher::sip::SipHasher13;

use crate::types::EmitMode;

/// Result of the `register_context_name` naming pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextNameResult {
    /// Symbol name used in generated code.
    /// Prod: `s_{hash}`, others: `{display_name_core}_{hash}`
    pub symbol_name: String,

    /// Display name for the segment (raw file_name prefix + display_name_core).
    /// Format: `{file_name}_{display_name_core}` (e.g., "test.tsx_test_component")
    pub display_name: String,

    /// 11-character hash.
    pub hash: String,

    /// Canonical filename: `{display_name}_{hash}`
    pub canonical_filename: String,
}

// ---------------------------------------------------------------------------
// compute_segment_hash
// ---------------------------------------------------------------------------

/// Compute the 11-character segment hash.
///
/// The `display_name` parameter here is the **pre-file-prefix** portion
/// (e.g., `"test_component"` NOT `"test.tsx_test_component"`).
/// The file_name is NOT included in the hash input — only the pre-prefix.
///
/// Algorithm:
///   - SipHasher13 with seed (0, 0)
///   - Write scope bytes (if Some)
///   - Write rel_path bytes
///   - Write display_name bytes (pre-file-prefix portion)
///   - Finish → u64 → to_le_bytes → base64 URL_SAFE_NO_PAD → replace `-`/`_` with `0`
pub(crate) fn compute_segment_hash(
    scope: Option<&str>,
    rel_path: &str,
    display_name: &str,
) -> String {
    let mut hasher = SipHasher13::new_with_keys(0, 0);
    if let Some(s) = scope {
        hasher.write(s.as_bytes());
    }
    hasher.write(rel_path.as_bytes());
    hasher.write(display_name.as_bytes());
    let hash_value = hasher.finish();

    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash_value.to_le_bytes());
    encoded.replace(['-', '_'], "0")
}

/// Compute hash, optionally short-circuiting via `hash_override`.
///
/// When `hash_override` is Some, ONLY its bytes are hashed (scope/rel_path/display_name
/// are ignored). Otherwise delegates to `compute_segment_hash`.
pub(crate) fn compute_segment_hash_with_override(
    scope: Option<&str>,
    rel_path: &str,
    display_name: &str,
    hash_override: Option<&str>,
) -> String {
    if let Some(h) = hash_override {
        // Hash override: write only the override bytes
        let mut hasher = SipHasher13::new_with_keys(0, 0);
        hasher.write(h.as_bytes());
        let hash_value = hasher.finish();
        let encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash_value.to_le_bytes());
        return encoded.replace(['-', '_'], "0");
    }
    compute_segment_hash(scope, rel_path, display_name)
}

// ---------------------------------------------------------------------------
// escape_sym
// ---------------------------------------------------------------------------

/// Normalize a string to a valid identifier fragment.
///
/// Steps:
///   1. Replace non-ASCII-alphanumeric chars with `_`
///   2. Squash consecutive underscores into one
///   3. Trim leading and trailing underscores
///   4. Prepend `_` if the result starts with a digit
///   5. Return empty string if input is empty or result is all underscores
pub(crate) fn escape_sym(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Step 1: Replace non-alphanumeric (ASCII) with '_'
    let replaced: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    // Step 2: Squash consecutive underscores
    let mut squashed = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if !prev_underscore {
                squashed.push(c);
            }
            prev_underscore = true;
        } else {
            squashed.push(c);
            prev_underscore = false;
        }
    }

    // Step 3: Trim leading and trailing underscores
    let trimmed = squashed.trim_matches('_');

    if trimmed.is_empty() {
        return String::new();
    }

    // Step 4: Digit prefix guard
    if trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// get_canonical_filename
// ---------------------------------------------------------------------------

/// Derive `canonical_filename` from `display_name` and `symbol_name`.
///
/// Extracts the hash suffix (last `_`-delimited token of symbol_name)
/// and returns `{display_name}_{hash_suffix}`.
pub(crate) fn get_canonical_filename(display_name: &str, symbol_name: &str) -> String {
    let hash_suffix = symbol_name
        .rsplit('_')
        .next()
        .unwrap_or(symbol_name);
    format!("{display_name}_{hash_suffix}")
}

// ---------------------------------------------------------------------------
// register_context_name
// ---------------------------------------------------------------------------

/// Full 6-step naming pipeline for a segment.
///
/// # Arguments
/// - `stack_ctxt` — stack of context strings (joined with `_` to form the base name)
/// - `segment_names` — mutable collision counter map (`display_name_core` → count)
/// - `scope` — optional package scope prefix
/// - `rel_path` — relative path of source file
/// - `file_name` — RAW filename with extension (e.g., `"test.tsx"`) — NOT escaped
/// - `mode` — emit mode (Prod uses mangled `s_{hash}` symbol names)
/// - `custom_symbol` — if Some, bypasses stack_ctxt derivation entirely
/// - `display_name_override` — if Some, used as display_name_core for hash (but not for naming)
/// - `hash_override` — if Some, bypasses normal hash computation
///
/// # Steps
/// 1. Custom symbol short-circuit (if custom_symbol is Some)
/// 1b. Join stack_ctxt with `_`, run through escape_sym
/// 2. Collision counter — append `_{n-1}` suffix for duplicate names (none for first)
/// 3. Compute hash via compute_segment_hash_with_override
/// 4. Build symbol_name based on mode (Prod: `s_{hash}`, others: `{core}_{hash}`)
/// 5. Build display_name = `{file_name}_{display_name_core}`
/// 6. Build canonical_filename via get_canonical_filename
pub(crate) fn register_context_name(
    stack_ctxt: &[String],
    segment_names: &mut HashMap<String, u32>,
    scope: Option<&str>,
    rel_path: &str,
    file_name: &str,
    mode: &EmitMode,
    custom_symbol: Option<&str>,
    display_name_override: Option<&str>,
    hash_override: Option<&str>,
) -> ContextNameResult {
    // Step 1: custom_symbol short-circuit
    if let Some(sym) = custom_symbol {
        let hash = compute_segment_hash_with_override(scope, rel_path, sym, hash_override);
        let symbol_name = match mode {
            EmitMode::Prod => format!("s_{hash}"),
            _ => format!("{sym}_{hash}"),
        };
        let display_name = format!("{file_name}_{sym}");
        let canonical_filename = get_canonical_filename(&display_name, &symbol_name);
        return ContextNameResult {
            symbol_name,
            display_name,
            hash,
            canonical_filename,
        };
    }

    // Step 1b: join stack_ctxt and escape
    let joined = stack_ctxt.join("_");
    let base_core = escape_sym(&joined);

    // Step 2: collision counter
    // Behavior: first occurrence → no suffix, second → _1, third → _2, etc.
    // counter=0 → no suffix, counter=1 → "_1", counter=2 → "_2"
    let counter = segment_names.entry(base_core.clone()).or_insert(0);
    let display_name_core = if *counter == 0 {
        base_core.clone()
    } else {
        format!("{base_core}_{}", *counter)
    };
    *counter += 1;

    // Step 3: hash
    let hash_input = display_name_override.unwrap_or(&display_name_core);
    let hash = compute_segment_hash_with_override(scope, rel_path, hash_input, hash_override);

    // Step 4: symbol_name
    let symbol_name = match mode {
        EmitMode::Prod => format!("s_{hash}"),
        _ => format!("{display_name_core}_{hash}"),
    };

    // Step 5: display_name — file_name is RAW (not escape_sym'd)
    let display_name = format!("{file_name}_{display_name_core}");

    // Step 6: canonical_filename
    let canonical_filename = get_canonical_filename(&display_name, &symbol_name);

    ContextNameResult {
        symbol_name,
        display_name,
        hash,
        canonical_filename,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // compute_segment_hash — golden snapshot vectors
    // -----------------------------------------------------------------------

    #[test]
    fn hash_golden_test_component() {
        // From destructure_args_colon_props.snap
        // display_name stored: "test.tsx_test_component", hash: "LUXeXe0DQrg"
        // The hash input is the pre-file-prefix portion: "test_component"
        let h = compute_segment_hash(None, "test.tsx", "test_component");
        assert_eq!(h, "LUXeXe0DQrg", "test_component hash mismatch");
    }

    #[test]
    fn hash_golden_foo_component() {
        // From component_level_self_referential_qrl.snap
        // hash: "HTDRsvUbLiE"
        let h = compute_segment_hash(None, "test.tsx", "Foo_component");
        assert_eq!(h, "HTDRsvUbLiE", "Foo_component hash mismatch");
    }

    #[test]
    fn hash_golden_foo_component_sig_use_async() {
        // From component_level_self_referential_qrl.snap
        // hash: "f0BGwWm4eeY"
        let h = compute_segment_hash(None, "test.tsx", "Foo_component_sig_useAsync");
        assert_eq!(h, "f0BGwWm4eeY", "Foo_component_sig_useAsync hash mismatch");
    }

    #[test]
    fn hash_golden_foo_component_other_use_async() {
        // From component_level_self_referential_qrl.snap
        // hash: "fsHooibmyyE"
        let h = compute_segment_hash(None, "test.tsx", "Foo_component_other_useAsync");
        assert_eq!(h, "fsHooibmyyE", "Foo_component_other_useAsync hash mismatch");
    }

    #[test]
    fn hash_always_11_chars() {
        let h = compute_segment_hash(None, "test.tsx", "test_component");
        assert_eq!(h.len(), 11, "hash must be 11 chars, got {h:?}");
    }

    #[test]
    fn hash_alphanumeric_only() {
        let h = compute_segment_hash(None, "test.tsx", "test_component");
        assert!(
            h.chars().all(|c| c.is_ascii_alphanumeric()),
            "hash must be alphanumeric (no - or _), got {h:?}"
        );
    }

    #[test]
    fn hash_scope_changes_output() {
        let without_scope = compute_segment_hash(None, "test.tsx", "test_component");
        let with_scope = compute_segment_hash(Some("my-pkg"), "test.tsx", "test_component");
        assert_ne!(without_scope, with_scope, "scope must change the hash");
    }

    #[test]
    fn hash_deterministic() {
        let h1 = compute_segment_hash(None, "test.tsx", "test_component");
        let h2 = compute_segment_hash(None, "test.tsx", "test_component");
        assert_eq!(h1, h2, "hash must be deterministic");
    }

    #[test]
    fn hash_override_bypasses_normal_inputs() {
        let normal = compute_segment_hash_with_override(
            None,
            "test.tsx",
            "test_component",
            None,
        );
        // With override — scope/rel_path/display_name are ignored
        let overridden = compute_segment_hash_with_override(
            Some("some-scope"),
            "completely-different.tsx",
            "different_name",
            Some("my-override"),
        );
        // Two different overrides produce different hashes
        let overridden2 = compute_segment_hash_with_override(
            Some("some-scope"),
            "completely-different.tsx",
            "different_name",
            Some("other-override"),
        );
        assert_ne!(normal, overridden, "override should change output vs no-override");
        assert_ne!(overridden, overridden2, "different overrides should produce different hashes");
        // Same override always produces the same result regardless of other inputs
        let overridden3 = compute_segment_hash_with_override(
            None,
            "test.tsx",
            "test_component",
            Some("my-override"),
        );
        assert_eq!(overridden, overridden3, "same override bytes must produce same hash");
    }

    // -----------------------------------------------------------------------
    // escape_sym
    // -----------------------------------------------------------------------

    #[test]
    fn escape_sym_replaces_non_alnum_with_underscore() {
        assert_eq!(escape_sym("my-component.handler"), "my_component_handler");
    }

    #[test]
    fn escape_sym_trims_leading_underscores() {
        assert_eq!(escape_sym("---foo"), "foo");
    }

    #[test]
    fn escape_sym_digit_prefix() {
        assert_eq!(escape_sym("123click"), "_123click");
    }

    #[test]
    fn escape_sym_already_valid() {
        assert_eq!(escape_sym("already_valid"), "already_valid");
    }

    #[test]
    fn escape_sym_empty() {
        assert_eq!(escape_sym(""), "");
    }

    #[test]
    fn escape_sym_all_special_chars() {
        // All underscores after replacement → trimmed → empty
        assert_eq!(escape_sym("---"), "");
    }

    // -----------------------------------------------------------------------
    // get_canonical_filename
    // -----------------------------------------------------------------------

    #[test]
    fn canonical_filename_extracts_hash_from_symbol() {
        // canonical_filename("test.tsx_test_component", "test_component_LUXeXe0DQrg")
        //   == "test.tsx_test_component_LUXeXe0DQrg"
        let cf = get_canonical_filename(
            "test.tsx_test_component",
            "test_component_LUXeXe0DQrg",
        );
        assert_eq!(cf, "test.tsx_test_component_LUXeXe0DQrg");
    }

    #[test]
    fn canonical_filename_handles_nested_segment() {
        // Nested: symbol_name = "Foo_component_sig_useAsync_f0BGwWm4eeY"
        let cf = get_canonical_filename(
            "test.tsx_Foo_component_sig_useAsync",
            "Foo_component_sig_useAsync_f0BGwWm4eeY",
        );
        assert_eq!(cf, "test.tsx_Foo_component_sig_useAsync_f0BGwWm4eeY");
    }

    // -----------------------------------------------------------------------
    // register_context_name
    // -----------------------------------------------------------------------

    #[test]
    fn register_context_name_basic_dev_mode() {
        let mut names: HashMap<String, u32> = HashMap::new();
        let stack = vec!["test".to_string(), "component".to_string()];
        let result = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );
        // display_name_core = escape_sym("test_component") = "test_component"
        // hash(None, "test.tsx", "test_component") = "LUXeXe0DQrg"
        assert_eq!(result.hash, "LUXeXe0DQrg");
        assert_eq!(result.display_name, "test.tsx_test_component");
        assert_eq!(result.symbol_name, "test_component_LUXeXe0DQrg");
        assert_eq!(result.canonical_filename, "test.tsx_test_component_LUXeXe0DQrg");
    }

    #[test]
    fn register_context_name_prod_mode_uses_mangled_symbol() {
        let mut names: HashMap<String, u32> = HashMap::new();
        let stack = vec!["test".to_string(), "component".to_string()];
        let result = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Prod,
            None,
            None,
            None,
        );
        // Prod: symbol_name = "s_{hash}"
        assert_eq!(result.symbol_name, format!("s_{}", result.hash));
        assert_eq!(result.display_name, "test.tsx_test_component");
    }

    #[test]
    fn register_context_name_collision_counter() {
        let mut names: HashMap<String, u32> = HashMap::new();
        let stack = vec!["test".to_string(), "component".to_string()];

        // First call — no suffix
        let r1 = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );
        // Second call — _1 suffix (counter was 1, suffix = counter-1 = 0? No: see spec)
        // Spec: first=none, second=_1, third=_2
        // counter starts at 0, first call: counter=0 → no suffix, increment to 1
        //                     second call: counter=1 → suffix "_0"... wait
        // Re-read: "If 0: no suffix. If >0: append _{counter-1}. Increment after."
        // First: counter=0 → no suffix, counter becomes 1
        // Second: counter=1 → suffix _{1-1} = _0...
        // But plan says: first=none, second=_1, third=_2
        // So the plan says something different. Let me use the spec:
        // "second identical ctx produces _1 suffix, third _2"
        // This means: first → no suffix, second → _1, third → _2
        // So the counter logic must be: counter 0→no suffix, counter 1→_1, counter 2→_2
        // i.e., counter IS the suffix number (not counter-1)
        // But plan says "If 0: no suffix. If >0: append _{counter-1}. Increment after."
        // That's contradictory. Let me use the behavior spec as truth:
        // first=none, second=_1, third=_2
        // → counter 0 → no suffix → increment to 1
        //   counter 1 → "_1" suffix → increment to 2
        //   counter 2 → "_2" suffix → increment to 3
        // So: suffix = counter (when counter > 0), not counter-1
        // I'll update the impl to match the behavior spec
        let r2 = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );
        let r3 = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );

        // First: no suffix → base core = "test_component"
        assert!(
            r1.display_name.ends_with("_test_component"),
            "first should have no suffix, got: {}",
            r1.display_name
        );
        // Second: _1 suffix → display_name_core = "test_component_1"
        assert!(
            r2.display_name.ends_with("_test_component_1"),
            "second should have _1 suffix, got: {}",
            r2.display_name
        );
        // Third: _2 suffix
        assert!(
            r3.display_name.ends_with("_test_component_2"),
            "third should have _2 suffix, got: {}",
            r3.display_name
        );
    }

    #[test]
    fn register_context_name_file_name_is_raw() {
        // file_name should be raw "test.tsx", NOT "test_tsx"
        let mut names: HashMap<String, u32> = HashMap::new();
        let stack = vec!["component".to_string()];
        let result = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );
        assert!(
            result.display_name.starts_with("test.tsx_"),
            "display_name must use raw file_name with extension, got: {}",
            result.display_name
        );
    }

    #[test]
    fn register_context_name_canonical_filename_is_display_name_plus_hash() {
        let mut names: HashMap<String, u32> = HashMap::new();
        let stack = vec!["test".to_string(), "component".to_string()];
        let result = register_context_name(
            &stack,
            &mut names,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None,
            None,
            None,
        );
        assert_eq!(
            result.canonical_filename,
            format!("{}_{}", result.display_name, result.hash)
        );
    }
}
