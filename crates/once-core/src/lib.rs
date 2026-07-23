//! Action types and cache-aware execution.
//!
//! Exposes command actions, portable file actions, and an async
//! executor ([`Runner`]) that consults a cache provider before doing
//! work. Subprocess output is streamed through the CAS rather than
//! buffered.

mod action;
mod contract;
mod directory_blob;
mod env;
mod error;
mod evidence;
mod execute;
mod execution_path;
mod file_blob;
mod input_digest;
mod local;
mod outputs;
mod path;
mod plan;
mod remote;
mod resources;
mod runner;
mod store;
mod stream;
mod test_manifest;
mod test_plan;
mod test_results;
mod test_schedule;
mod xdg;

pub use action::{
    Action, CopyPathMode, OutputSymlinkMode, PreparePathMode, RemoteExecution, SandboxMode,
};
pub use contract::{
    validate_action_contract, validate_action_contract_with_options, ActionContractDiagnostic,
    ActionContractOptions, ActionContractReport, FilesystemOperation,
};
pub use contract::{ContractViolation, ContractViolationKind};
pub use env::{
    managed_mise, managed_mise_path, select_tool_env, tool_env, workspace_executable,
    workspace_has_mise_config, workspace_mise_command, workspace_mise_env, workspace_prepare_tools,
    workspace_tool, workspace_tool_command, workspace_tool_env,
    workspace_tool_env_with_executables, workspace_tool_var, ToolEnvError, MANAGED_MISE_VERSION,
};
pub use error::{Error, Result};
pub use evidence::{
    EvidenceCacheState, EvidenceRecord, EvidenceStatus, EvidenceStore, EvidenceSubject,
};
pub use execution_path::{
    resolve_execution_argv, resolve_execution_env, resolve_execution_value, EXECUTION_ROOT_MARKER,
};
pub use input_digest::InputDigestBuilder;
pub use path::{WorkspacePath, WorkspacePathError};
pub use plan::{BuiltPlan, NodeInfo, Plan, PlanError, PlanNode, PlanOutcome};
pub use resources::{ResourceLimits, ResourcePool, ResourceRequest};
pub use runner::{
    run, run_uncached, run_uncached_contract, run_with_cache, run_with_cache_streaming, CacheState,
    Outcome, RunOpts, Runner,
};
pub use store::WorkspaceStore;
pub use test_manifest::{TestManifest, TestSharding, TestUnit, TEST_MANIFEST_SCHEMA};
pub use test_plan::{
    SelectedTest, TestBatch, TestPlan, TestSelectionPolicy, TestSelectionReport, TEST_PLAN_SCHEMA,
    TEST_SELECTION_SCHEMA,
};
pub use test_results::{
    validate_test_results, validate_test_results_for_units, TEST_RESULTS_SCHEMA,
};
pub use test_schedule::{
    TestBatchAttempt, TestBatchAttemptSpec, TestBatchStatus, TestSchedule, TestTimingStore,
    TEST_BATCH_ATTEMPT_SCHEMA, TEST_SCHEDULE_SCHEMA,
};
pub use xdg::Xdg;
