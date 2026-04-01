//! Qwik optimizer using OXC for code transformation.
//!
//! This crate provides the type definitions and utility functions needed for
//! the Qwik $-call extraction pipeline. Full transform logic is added in
//! subsequent phases.

pub mod hash;

mod errors;
mod is_const;
pub(crate) mod parse;
mod types;
mod words;

pub(crate) mod entry_strategy;

// Re-export all public types
pub use types::{
    CtxKind,
    Diagnostic,
    DiagnosticCategory,
    EmitMode,
    EntryStrategy,
    MinifyMode,
    QwikBundle,
    QwikManifest,
    SegmentAnalysis,
    SourceLocation,
    TransformModule,
    TransformModuleInput,
    TransformModulesOptions,
    TransformOutput,
};
