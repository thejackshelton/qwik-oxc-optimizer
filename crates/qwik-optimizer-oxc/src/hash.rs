//! Segment hash computation.
//!
//! Computes the 11-character hash that appears in segment names
//! (e.g., `zBbHWn4e8Cg` in `renderHeader_zBbHWn4e8Cg`).
//!
//! Algorithm: `DefaultHasher(scope?, rel_path, display_name) -> u64 -> LE bytes -> base64url -> replace -/_ with 0`
//! This is an exact port of the SWC optimizer hash algorithm validated in POC-03.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use base64::Engine;

/// Compute the segment hash for a display name.
///
/// Returns an 11-character base64url hash. The exact algorithm matches the
/// SWC optimizer: feed scope (if any), rel_path, and display_name bytes into
/// a `DefaultHasher`, encode the u64 result as LE bytes in base64url, and
/// replace `-` and `_` with `0`.
///
/// # Arguments
/// - `scope` - Optional scope prefix (e.g., package name for monorepos)
/// - `rel_path` - Relative file path (e.g., "test.tsx")
/// - `display_name` - Full display name (e.g., "test.tsx_renderHeader")
pub(crate) fn compute_segment_hash(
    scope: Option<&str>,
    rel_path: &str,
    display_name: &str,
) -> String {
    // Normalize Windows backslashes to forward slashes before hashing.
    // SWC uses `rel_path.to_slash_lossy()` which converts backslashes,
    // so the hash input must use forward slashes to match.
    let normalized_path = rel_path.replace('\\', "/");
    let mut hasher = DefaultHasher::new();
    if let Some(scope) = scope {
        hasher.write(scope.as_bytes());
    }
    hasher.write(normalized_path.as_bytes());
    hasher.write(display_name.as_bytes());
    let hash = hasher.finish();

    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.to_le_bytes());
    encoded.replace(['-', '_'], "0")
}

/// Format a full segment name from a display name and hash.
///
/// Example: `format_segment_name("renderHeader", "zBbHWn4e8Cg")` -> `"renderHeader_zBbHWn4e8Cg"`
pub(crate) fn format_segment_name(display_name: &str, hash: &str) -> String {
    format!("{display_name}_{hash}")
}
