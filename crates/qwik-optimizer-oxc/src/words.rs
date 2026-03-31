//! String constants and dollar API helpers.
//!
//! Provides helpers for classifying dollar API call sites by context kind
//! and converting $-suffixed names to their Qrl-suffixed equivalents.

use crate::types::CtxKind;

/// Convert a $-suffixed name to its Qrl-suffixed equivalent.
///
/// Example: "component$" -> "componentQrl"
pub(crate) fn dollar_to_qrl_name(name: &str) -> String {
    name.strip_suffix('$')
        .map(|s| format!("{s}Qrl"))
        .unwrap_or_else(|| name.to_string())
}

/// Classify the context kind of a dollar call site.
///
/// Returns `CtxKind::EventHandler` for:
/// - `event$` (explicit event handler API)
/// - JSX event handler attribute names: `on[A-Z]*$` (e.g., `onClick$`, `onInput$`)
/// - Namespaced JSX event handlers: `document:onClick$`, `window:onFocus$`
///
/// Returns `CtxKind::Function` for everything else:
/// - `$`, `component$`, `useTask$`, `useStyles$`, `useVisibleTask$`, etc.
///
/// This matches the SWC optimizer behavior where `component$`, `useTask$`,
/// and bare `$` all get `Function`, while JSX `onClick$` attributes get
/// `EventHandler`.
pub(crate) fn classify_ctx_kind(callee_name: &str) -> CtxKind {
    // Strip any namespace prefix (e.g., "document:onClick$" -> "onClick$")
    let base_name = if let Some(pos) = callee_name.find(':') {
        &callee_name[pos + 1..]
    } else {
        callee_name
    };

    if base_name == "event$" {
        return CtxKind::EventHandler;
    }

    // Check for on[A-Z]*$ pattern (JSX event handler attributes)
    if base_name.starts_with("on") && base_name.ends_with('$') && base_name.len() > 3 {
        // Verify the third character is uppercase (on[A-Z]...)
        if let Some(ch) = base_name.chars().nth(2) {
            if ch.is_ascii_uppercase() || ch == '-' {
                return CtxKind::EventHandler;
            }
        }
    }

    CtxKind::Function
}
