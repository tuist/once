//! Durable evidence records for action outcomes.
//!
//! Evidence is provenance, not cache. The action cache answers whether
//! an output can be reused; evidence records what happened so humans
//! and agents can query recent engineering state without scraping
//! command output.

mod entity;
pub(crate) mod migration;
mod record;
mod store;

pub use record::{EvidenceCacheState, EvidenceRecord, EvidenceStatus, EvidenceSubject};
pub use store::EvidenceStore;
