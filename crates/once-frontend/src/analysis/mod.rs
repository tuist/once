//! Starlark rule implementation analysis.
//!
//! Rule schemas declared in the prelude carry an optional `impl`
//! callable. The analysis pass evaluates that callable for one target
//! at a time with a `ctx` dict and collects the actions the impl
//! declares through generic globals.
//!
//! The exported Rust surface is deliberately generic. Toolchain
//! discovery, SDK names, triples, binary formats, and source filters
//! live in Starlark rule files, while Rust provides execution and data
//! plumbing.

mod engine;
mod globals;
mod store;
mod values;

use std::collections::BTreeMap;

use crate::target::AttrValue;

pub use engine::{analyze_target, rule_has_impl, AnalysisEngine, AnalysisResult};
pub use globals::globals_for_prelude;
pub use store::{with_active_store, AnalysisStore, DeclaredAction};

/// If `value` is the canonical select-shape Map (`{ "select": { ... }
/// }`), return the inner branch map. Otherwise return `None`. The
/// resolution mechanism itself lives in the Starlark prelude so that
/// rule-family-specific configuration knowledge (which attributes
/// feed the configuration, which token names are recognised) stays
/// out of the Rust executor; this helper exists only so the graph
/// schema layer can flag selects on `configurable = False` attributes
/// before the prelude ever runs.
#[must_use]
pub fn select_branches(value: &AttrValue) -> Option<&BTreeMap<String, AttrValue>> {
    if let AttrValue::Map(map) = value {
        if map.len() == 1 {
            if let Some(AttrValue::Map(branches)) = map.get("select") {
                return Some(branches);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
