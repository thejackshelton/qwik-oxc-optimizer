//! EntryStrategy application.
//!
//! Apply the configured `EntryStrategy` to determine how segments are output.
//! The strategy controls whether segments become separate files (Segment),
//! stay inline (Inline), are hoisted, etc.

use crate::types::EntryStrategy;

/// Determine whether the strategy results in inline segments.
pub(crate) fn should_inline(strategy: &EntryStrategy) -> bool {
    matches!(strategy, EntryStrategy::Inline)
}
