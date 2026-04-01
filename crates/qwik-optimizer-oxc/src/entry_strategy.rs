//! Entry policy trait and strategy implementations.
//!
//! Each `EntryStrategy` variant maps to an `EntryPolicy` implementation via
//! `parse_entry_strategy()`. The policy's single method `get_entry_for_sym`
//! determines the output chunk key for a given segment.
//!
//! SPEC reference: Chapter 1 §"The EntryPolicy Trait" (lines 626–775)

use crate::types::{CtxKind, EntryStrategy, SegmentData};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Controls how extracted segments are grouped into output chunks.
///
/// SPEC: `get_entry_for_sym` returns `Some(key)` to group this segment with
/// all other segments sharing the same key, or `None` to give it its own chunk.
pub(crate) trait EntryPolicy: Send + Sync {
    fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Policy implementations
// ---------------------------------------------------------------------------

/// Inline / Hoist strategy: all segments share the "entry_segments" chunk.
struct InlineStrategy;

impl EntryPolicy for InlineStrategy {
    fn get_entry_for_sym(&self, _context: &[String], _segment: &SegmentData) -> Option<String> {
        Some("entry_segments".to_string())
    }
}

/// Single strategy: all segments share the "entry_segments" chunk.
struct SingleStrategy;

impl EntryPolicy for SingleStrategy {
    fn get_entry_for_sym(&self, _context: &[String], _segment: &SegmentData) -> Option<String> {
        Some("entry_segments".to_string())
    }
}

/// Hook / Segment strategy: every segment gets its own chunk.
struct PerSegmentStrategy;

impl EntryPolicy for PerSegmentStrategy {
    fn get_entry_for_sym(&self, _context: &[String], _segment: &SegmentData) -> Option<String> {
        None
    }
}

/// Component strategy: group by root component name, or "entry_segments" for top-level QRLs.
struct PerComponentStrategy;

impl EntryPolicy for PerComponentStrategy {
    fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<String> {
        context.first().map_or_else(
            || Some("entry_segments".to_string()),
            |root| Some(format!("{}_entry_{}", segment.origin, root)),
        )
    }
}

/// Smart strategy: heuristic that owns pure event handlers and groups the rest per-component.
struct SmartStrategy;

impl EntryPolicy for SmartStrategy {
    fn get_entry_for_sym(&self, context: &[String], segment: &SegmentData) -> Option<String> {
        // Branch 1: pure event handlers (no scope captures) → own chunk
        if segment.scoped_idents.is_empty()
            && (segment.ctx_kind != CtxKind::Function || segment.ctx_name == "event$")
        {
            return None;
        }
        // Branch 2: top-level QRLs (no context) → own chunk
        // Branch 3: otherwise → per-component grouping
        context.first().map_or(None, |root| {
            Some(format!("{}_entry_{}", segment.origin, root))
        })
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Map an `EntryStrategy` variant to its policy implementation.
///
/// SPEC: lines 624–651
pub(crate) fn parse_entry_strategy(strategy: &EntryStrategy) -> Box<dyn EntryPolicy> {
    match strategy {
        EntryStrategy::Inline | EntryStrategy::Hoist => Box::new(InlineStrategy),
        EntryStrategy::Single => Box::new(SingleStrategy),
        EntryStrategy::Hook | EntryStrategy::Segment => Box::new(PerSegmentStrategy),
        EntryStrategy::Component => Box::new(PerComponentStrategy),
        EntryStrategy::Smart => Box::new(SmartStrategy),
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Returns `true` only for `Inline` and `Hoist`.
///
/// `Single` maps to the same "entry_segments" key but does NOT trigger the
/// `SideEffectVisitor` pass — this gate is intentionally exclusive.
///
/// SPEC: lines 769–775
pub(crate) fn is_inline(strategy: &EntryStrategy) -> bool {
    matches!(strategy, EntryStrategy::Inline | EntryStrategy::Hoist)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CtxKind, EntryStrategy, SegmentData};

    // ------------------------------------------------------------------
    // Helper: build a minimal SegmentData for policy tests
    // ------------------------------------------------------------------
    fn seg(origin: &str, ctx_kind: CtxKind, ctx_name: &str, scoped_idents: Vec<String>) -> SegmentData {
        SegmentData {
            origin: origin.to_string(),
            ctx_kind,
            ctx_name: ctx_name.to_string(),
            scoped_idents,
            // Remaining fields are irrelevant for entry-policy tests
            display_name: String::new(),
            hash: String::new(),
            name: String::new(),
            extension: String::new(),
            span: (0, 0),
            parent: None,
            captures: false,
            capture_names: vec![],
            needed_imports: vec![],
            segment_qrl_names: vec![],
            body_span: (0, 0),
            param_names: vec![],
            body_code: String::new(),
            child_lazy_imports: vec![],
            needs_qrl_import: false,
        }
    }

    // ------------------------------------------------------------------
    // is_inline
    // ------------------------------------------------------------------

    #[test]
    fn is_inline_inline_true() {
        assert!(is_inline(&EntryStrategy::Inline));
    }

    #[test]
    fn is_inline_hoist_true() {
        assert!(is_inline(&EntryStrategy::Hoist));
    }

    #[test]
    fn is_inline_single_false() {
        // Single does NOT trigger the SideEffectVisitor gate
        assert!(!is_inline(&EntryStrategy::Single));
    }

    #[test]
    fn is_inline_segment_false() {
        assert!(!is_inline(&EntryStrategy::Segment));
    }

    #[test]
    fn is_inline_hook_false() {
        assert!(!is_inline(&EntryStrategy::Hook));
    }

    #[test]
    fn is_inline_component_false() {
        assert!(!is_inline(&EntryStrategy::Component));
    }

    #[test]
    fn is_inline_smart_false() {
        assert!(!is_inline(&EntryStrategy::Smart));
    }

    // ------------------------------------------------------------------
    // InlineStrategy (Inline + Hoist)
    // ------------------------------------------------------------------

    #[test]
    fn inline_strategy_always_entry_segments() {
        let policy = parse_entry_strategy(&EntryStrategy::Inline);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&[], &s),
            Some("entry_segments".to_string())
        );
        assert_eq!(
            policy.get_entry_for_sym(&["root".to_string()], &s),
            Some("entry_segments".to_string())
        );
    }

    #[test]
    fn hoist_strategy_maps_to_inline_strategy() {
        let policy = parse_entry_strategy(&EntryStrategy::Hoist);
        let s = seg("test", CtxKind::EventHandler, "$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&[], &s),
            Some("entry_segments".to_string())
        );
    }

    // ------------------------------------------------------------------
    // SingleStrategy
    // ------------------------------------------------------------------

    #[test]
    fn single_strategy_always_entry_segments() {
        let policy = parse_entry_strategy(&EntryStrategy::Single);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&[], &s),
            Some("entry_segments".to_string())
        );
        assert_eq!(
            policy.get_entry_for_sym(&["root".to_string()], &s),
            Some("entry_segments".to_string())
        );
    }

    // ------------------------------------------------------------------
    // PerSegmentStrategy (Hook + Segment)
    // ------------------------------------------------------------------

    #[test]
    fn segment_strategy_always_none() {
        let policy = parse_entry_strategy(&EntryStrategy::Segment);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(policy.get_entry_for_sym(&[], &s), None);
        assert_eq!(
            policy.get_entry_for_sym(&["root".to_string()], &s),
            None
        );
    }

    #[test]
    fn hook_strategy_always_none() {
        let policy = parse_entry_strategy(&EntryStrategy::Hook);
        let s = seg("test", CtxKind::EventHandler, "$", vec![]);
        assert_eq!(policy.get_entry_for_sym(&[], &s), None);
    }

    // ------------------------------------------------------------------
    // PerComponentStrategy
    // ------------------------------------------------------------------

    #[test]
    fn component_strategy_no_context_returns_entry_segments() {
        let policy = parse_entry_strategy(&EntryStrategy::Component);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&[], &s),
            Some("entry_segments".to_string())
        );
    }

    #[test]
    fn component_strategy_with_context_returns_origin_entry_root() {
        let policy = parse_entry_strategy(&EntryStrategy::Component);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&["Header".to_string()], &s),
            Some("test_entry_Header".to_string())
        );
    }

    // ------------------------------------------------------------------
    // SmartStrategy
    // ------------------------------------------------------------------

    #[test]
    fn smart_pure_event_handler_no_captures_returns_none() {
        // Branch 1: scoped_idents empty AND ctx_kind != Function → None
        let policy = parse_entry_strategy(&EntryStrategy::Smart);
        let s = seg("test", CtxKind::EventHandler, "onClick$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&["Header".to_string()], &s),
            None
        );
    }

    #[test]
    fn smart_event_dollar_function_no_captures_returns_none() {
        // Branch 1 special case: ctx_kind == Function AND ctx_name == "event$" → None
        let policy = parse_entry_strategy(&EntryStrategy::Smart);
        let s = seg("test", CtxKind::Function, "event$", vec![]);
        assert_eq!(
            policy.get_entry_for_sym(&["Header".to_string()], &s),
            None
        );
    }

    #[test]
    fn smart_function_with_captures_and_context_returns_origin_entry_root() {
        // Branch 3: scoped_idents non-empty, context present → per-component
        let policy = parse_entry_strategy(&EntryStrategy::Smart);
        let s = seg(
            "test",
            CtxKind::Function,
            "useTask$",
            vec!["store".to_string()],
        );
        assert_eq!(
            policy.get_entry_for_sym(&["Header".to_string()], &s),
            Some("test_entry_Header".to_string())
        );
    }

    #[test]
    fn smart_function_no_captures_not_event_dollar_no_context_returns_none() {
        // Branch 1 fails (ctx_kind == Function, ctx_name != "event$") →
        // scoped_idents empty means branch 1 condition: empty AND (Function || "event$").
        // Since ctx_kind == Function AND ctx_name != "event$", branch 1 does NOT fire.
        // Then branch 2: no context → None
        let policy = parse_entry_strategy(&EntryStrategy::Smart);
        let s = seg("test", CtxKind::Function, "component$", vec![]);
        assert_eq!(policy.get_entry_for_sym(&[], &s), None);
    }

    #[test]
    fn smart_scoped_idents_check_before_context_check() {
        // Verify branch order: if scoped_idents is empty AND pure handler,
        // result is None EVEN IF context is present.
        let policy = parse_entry_strategy(&EntryStrategy::Smart);
        let s = seg("test", CtxKind::EventHandler, "onClick$", vec![]);
        // Has context, but pure handler → None (branch 1 fires first)
        assert_eq!(
            policy.get_entry_for_sym(&["Root".to_string()], &s),
            None
        );
    }
}
