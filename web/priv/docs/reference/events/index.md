# Live run event protocol

This reference is generated from the versioned wire definition during the documentation build. It describes the service, event variants, fields, and enum values shipped by Once.

The client publishes ordered batches and the service acknowledges the contiguous durable frontier. The server may return a dashboard link after run creation.

## service `RunEventService`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `rpc GetServerCapabilities(GetServerCapabilitiesRequest) returns (ServerCapabilities)` | call | |  |
| `rpc GetArgvHashKey(GetArgvHashKeyRequest) returns (ArgvHashKey)` | call | |  |
| `rpc PublishRunEvents(stream RunEventBatch) returns (stream BatchAck)` | call | |  |
| `rpc GetRunAck(GetRunAckRequest) returns (RunEventAck)` | call | |  |

## message `GetServerCapabilitiesRequest`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |

## message `GetRunAckRequest`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `run_id` | `string` | 1 |  |

## message `GetArgvHashKeyRequest`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `project_id` | `string` | 1 |  |

## message `ServerCapabilities`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `supported_protocol_versions` | `repeated string` | 1 |  |
| `max_batch_bytes` | `uint32` | 2 |  |
| `max_event_bytes` | `uint32` | 3 |  |
| `max_unacked_events` | `uint32` | 4 |  |
| `max_log_chunk_bytes` | `uint32` | 5 |  |
| `required_features` | `repeated string` | 6 |  |
| `log_ingestion_available` | `bool` | 7 |  |
| `raw_event_retention_available` | `bool` | 8 |  |
| `finalization_grace_ms` | `uint32` | 9 |  |
| `dedup_retention_seconds` | `uint32` | 10 |  |
| `safe_literal_allowlist_version` | `string` | 11 |  |

## message `ArgvHashKey`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `key_id` | `string` | 1 |  |
| `key_bytes` | `bytes` | 2 |  |
| `expires_at_epoch_ms` | `int64` | 3 |  |
| `grace_after_expiry_ms` | `uint32` | 4 |  |

## message `RunEventBatch`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `run_id` | `string` | 1 |  |
| `batch_id` | `string` | 2 |  |
| `gap_advances` | `repeated GapAdvance` | 3 |  |
| `seq_from` | `uint64` | 4 |  |
| `events` | `repeated RunEvent` | 5 |  |
| `producer_dropped_events` | `uint64` | 6 | Cumulative count dropped by the producer's event bus before assignment of sequence numbers. Unlike gap_advances, these have no sequence range. |

## message `GapAdvance`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `first_dropped_seq` | `uint64` | 1 |  |
| `last_dropped_seq` | `uint64` | 2 |  |
| `reason` | `string` | 3 |  |

## message `BatchAck`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `run_id` | `string` | 1 |  |
| `batch_id` | `string` | 2 |  |
| `disposition` | `AckDisposition` | 3 |  |
| `acked_seq` | `uint64` | 4 |  |
| `expected_next_seq` | `uint64` | 5 |  |
| `observed_high_water_seq` | `uint64` | 6 |  |
| `retry_after_ms` | `uint32` | 7 |  |
| `max_in_flight_batches` | `uint32` | 8 |  |
| `finalization` | `RunFinalization` | 9 |  |
| `dashboard_url` | `string` | 10 | Canonical absolute dashboard link, available after durable run creation. |

## message `RunEventAck`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `run_id` | `string` | 1 |  |
| `acked_seq` | `uint64` | 2 |  |
| `expected_next_seq` | `uint64` | 3 |  |
| `observed_high_water_seq` | `uint64` | 4 |  |
| `finalization` | `RunFinalization` | 5 |  |
| `dashboard_url` | `string` | 6 |  |

## enum `AckDisposition`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `ACK_DISPOSITION_UNSPECIFIED` | value | 0 |  |
| `ACK_DISPOSITION_ACCEPTED` | value | 1 |  |
| `ACK_DISPOSITION_REJECTED_STALE` | value | 2 |  |
| `ACK_DISPOSITION_REJECTED_INVALID` | value | 3 |  |
| `ACK_DISPOSITION_NEEDS_RESYNC` | value | 4 |  |

## enum `RunFinalization`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `RUN_FINALIZATION_UNSPECIFIED` | value | 0 |  |
| `RUN_FINALIZATION_ACTIVE` | value | 1 |  |
| `RUN_FINALIZATION_FINALIZING` | value | 2 |  |
| `RUN_FINALIZATION_FINALIZED` | value | 3 |  |
| `RUN_FINALIZATION_FINALIZATION_PENDING` | value | 4 |  |
| `RUN_FINALIZATION_LOST` | value | 5 |  |

## message `ContentRef`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `hash_algorithm` | `HashAlgorithm` | 1 |  |
| `digest` | `bytes` | 2 | Raw digest bytes, not hexadecimal text. |
| `size_bytes` | `uint64` | 3 |  |
| `namespace` | `string` | 4 |  |
| `media_type` | `string` | 5 |  |

## enum `HashAlgorithm`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `HASH_ALGORITHM_UNSPECIFIED` | value | 0 |  |
| `HASH_ALGORITHM_BLAKE3` | value | 1 |  |
| `HASH_ALGORITHM_SHA256` | value | 2 |  |

## message `RunEvent`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `seq` | `uint64` | 1 |  |
| `epoch_ms` | `int64` | 2 |  |
| `mono_ns` | `int64` | 3 |  |
| *payload* | one of | | |
| `run_started` | `RunStarted` | 10 |  |
| `run_finalizing` | `RunFinalizing` | 11 |  |
| `run_completed` | `RunCompleted` | 12 |  |
| `run_heartbeat` | `RunHeartbeat` | 13 |  |
| `graph_compiled` | `GraphCompiled` | 20 |  |
| `target_instance` | `TargetInstance` | 21 |  |
| `target_queued` | `TargetQueued` | 30 |  |
| `target_started` | `TargetStarted` | 31 |  |
| `target_phase` | `TargetPhase` | 32 |  |
| `target_wait` | `TargetWait` | 33 |  |
| `target_completed` | `TargetCompleted` | 34 |  |
| `target_cancelled` | `TargetCancelled` | 35 |  |
| `target_retried` | `TargetRetried` | 36 |  |
| `action_completed` | `ActionCompleted` | 37 |  |
| `target_phase_completed` | `TargetPhaseCompleted` | 38 |  |
| `action_attempt_started` | `ActionAttemptStarted` | 39 |  |
| `action_attempt_completed` | `ActionAttemptCompleted` | 45 |  |
| `test_suite_started` | `TestSuiteStarted` | 40 |  |
| `test_suite_completed` | `TestSuiteCompleted` | 41 |  |
| `test_case_started` | `TestCaseStarted` | 42 |  |
| `test_case_completed` | `TestCaseCompleted` | 43 |  |
| `test_case_retried` | `TestCaseRetried` | 44 |  |
| `log_chunk` | `LogChunk` | 50 |  |
| `log_truncated` | `LogTruncated` | 51 |  |
| `cache_probe` | `CacheProbe` | 60 |  |
| `cache_miss_reason` | `CacheMissReason` | 61 |  |
| `cache_upload` | `CacheUpload` | 62 |  |
| `cache_download` | `CacheDownload` | 63 |  |
| `cache_store_reused` | `CacheStoreReused` | 64 |  |
| `artifact_published` | `ArtifactPublished` | 70 |  |
| `diagnostic_emitted` | `DiagnosticEmitted` | 71 |  |
| `system_sampled` | `SystemSampled` | 80 |  |

## message `RunStarted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `once_version` | `string` | 1 |  |
| `protocol_version` | `string` | 2 |  |
| `host_class` | `string` | 3 |  |
| `git_rev` | `string` | 4 |  |
| `git_dirty` | `bool` | 5 |  |
| `argv_normalized` | `repeated ArgvToken` | 6 |  |
| `argv_hash_key_id` | `string` | 7 |  |
| `safe_literal_allowlist_version` | `string` | 8 |  |
| `cwd_relative` | `string` | 9 |  |
| `env_fingerprint` | `string` | 10 |  |
| `root_graph_digest` | `ContentRef` | 11 |  |
| `project_id` | `string` | 12 |  |
| `effective_limits` | `EffectiveLimits` | 13 |  |
| `is_ci` | `bool` | 14 | Whether the run happened on CI rather than a developer machine. The dashboard splits runs on this the way it does for the other build systems, which all report the same distinction. |

## message `ArgvToken`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| *token* | one of | | |
| `safe_literal` | `string` | 1 |  |
| `flag_key` | `string` | 2 |  |
| `named_value` | `NamedValue` | 3 |  |
| `opaque_value_hash` | `string` | 4 |  |

## message `NamedValue`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `key` | `string` | 1 |  |
| `value_shape_hash` | `string` | 2 |  |

## message `EffectiveLimits`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `max_batch_bytes` | `uint32` | 1 |  |
| `max_event_bytes` | `uint32` | 2 |  |
| `max_unacked_events` | `uint32` | 3 |  |
| `max_log_chunk_bytes` | `uint32` | 4 |  |
| `log_ingestion_enabled` | `bool` | 5 |  |
| `raw_event_retention_enabled` | `bool` | 6 |  |

## message `RunFinalizing`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `declared_drain_ms` | `int64` | 1 |  |

## message `RunCompleted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `result` | `RunResult` | 1 |  |
| `cancellation_reason` | `string` | 2 |  |
| `wall_ms` | `int64` | 3 |  |
| `totals` | `RunTotals` | 4 |  |
| `producer_dropped_events` | `uint64` | 5 |  |

## enum `RunResult`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `RUN_RESULT_UNSPECIFIED` | value | 0 |  |
| `RUN_RESULT_SUCCEEDED` | value | 1 |  |
| `RUN_RESULT_FAILED` | value | 2 |  |
| `RUN_RESULT_CANCELLED` | value | 3 |  |
| `RUN_RESULT_TIMED_OUT` | value | 4 |  |
| `RUN_RESULT_INFRASTRUCTURE_ERROR` | value | 5 |  |

## message `RunTotals`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `targets_by_result` | `map<string, uint32>` | 1 |  |
| `cases_by_result` | `map<string, uint32>` | 2 |  |
| `cache_hit_rate` | `double` | 3 |  |
| `cache_bytes_downloaded` | `uint64` | 4 |  |
| `cache_bytes_uploaded` | `uint64` | 5 |  |
| `cache_bytes_saved` | `uint64` | 6 |  |

## message `RunHeartbeat`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |

## message `GraphCompiled`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `graph_digest` | `ContentRef` | 1 |  |
| `target_count` | `uint32` | 2 |  |
| `kind_histogram` | `map<string, uint32>` | 3 |  |
| `roots` | `repeated string` | 4 |  |

## message `TargetInstance`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_instance_id` | `string` | 1 |  |
| `target_id` | `string` | 2 |  |
| `configuration_digest` | `ContentRef` | 3 |  |

## message `TargetQueued`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `target_instance_id` | `string` | 2 |  |
| `kind` | `string` | 3 |  |
| `capability` | `string` | 4 |  |
| `action_digest` | `ContentRef` | 5 |  |
| `input_digest` | `ContentRef` | 6 |  |
| `dep_target_executions` | `repeated string` | 7 |  |
| `attempt` | `uint32` | 8 |  |

## message `TargetStarted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `worker_class` | `string` | 2 |  |

## message `TargetPhase`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `phase` | `Phase` | 2 |  |

## enum `Phase`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `PHASE_UNSPECIFIED` | value | 0 |  |
| `PHASE_QUEUED` | value | 1 |  |
| `PHASE_CACHE_CHECKING` | value | 2 |  |
| `PHASE_PREPARING` | value | 3 |  |
| `PHASE_EXECUTING` | value | 4 |  |
| `PHASE_CAPTURING` | value | 5 |  |
| `PHASE_PUBLISHING` | value | 6 |  |

## message `TargetWait`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `reason` | `WaitReason` | 2 |  |
| `blocking_target_execution_id` | `string` | 3 |  |
| `resource_kind` | `string` | 4 |  |

## enum `WaitReason`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `WAIT_REASON_UNSPECIFIED` | value | 0 |  |
| `WAIT_REASON_DEPENDENCY_WAIT` | value | 1 |  |
| `WAIT_REASON_RESOURCE_WAIT` | value | 2 |  |
| `WAIT_REASON_WORKER_WAIT` | value | 3 |  |
| `WAIT_REASON_THROTTLED` | value | 4 |  |
| `WAIT_REASON_INFRASTRUCTURE` | value | 5 |  |

## message `TargetCompleted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `result` | `TargetResult` | 2 |  |
| `was_cached` | `bool` | 3 |  |
| `exit_code` | `optional int32` | 4 |  |
| `evidence_digest` | `ContentRef` | 5 |  |
| `duration_ms` | `int64` | 6 |  |

## enum `TargetResult`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `TARGET_RESULT_UNSPECIFIED` | value | 0 |  |
| `TARGET_RESULT_SUCCEEDED` | value | 1 |  |
| `TARGET_RESULT_FAILED` | value | 2 |  |
| `TARGET_RESULT_SKIPPED` | value | 3 |  |
| `TARGET_RESULT_CANCELLED` | value | 4 |  |
| `TARGET_RESULT_TIMED_OUT` | value | 5 |  |
| `TARGET_RESULT_INFRASTRUCTURE_ERROR` | value | 6 |  |

## message `TargetCancelled`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `reason` | `string` | 2 |  |

## message `TargetRetried`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `previous_target_execution_id` | `string` | 1 |  |
| `new_target_execution_id` | `string` | 2 |  |
| `new_attempt` | `uint32` | 3 |  |
| `reason` | `string` | 4 |  |

## message `ActionCompleted`

Exactly one declared action outcome. Identity within a run is the tuple (target_execution_id, capability, action_index). The index is zero-based declaration order, never completion order; identifier is a display label. A new target attempt must use a new target_execution_id. Cached target reuse emits the retained actions with current-run timing, not old spans.

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `capability` | `string` | 2 | "build", "test", ... |
| `action_index` | `uint32` | 3 | position within the target's ordered action list |
| `identifier` | `string` | 4 | Starlark-declared identity, may be empty |
| `result` | `TargetResult` | 5 |  |
| `was_cached` | `bool` | 6 |  |
| `duration_ms` | `int64` | 7 |  |
| `exit_code` | `int32` | 8 |  |
| `start_at_epoch_ms` | `int64` | 9 | Wall-clock start time of the action, in epoch milliseconds. When present the projector prefers this over `envelope.epoch_ms - duration_ms`, which collapses many actions that all completed inside one millisecond bucket. |
| `worker_id` | `string` | 10 | Stable identifier of the worker (tokio task / OS thread / target execution scheduler) that ran this action. The dashboard groups rows in the flame graph by worker, matching Bazel's per-tid rendering. |
| `prepare_ms` | `int64` | 11 | Split of `duration_ms` into the two phases the CLI observes. `prepare_ms` covers arg files, input fingerprinting, and action digest; `execute_ms` covers cache probe + (on a miss) command execution and output upload. The server renders each as its own sub-span on the worker's lane so a cache-heavy build shows what filled the wall clock instead of a sea of 1 ms dashes. Zero when unknown (cached-replay actions). |
| `execute_ms` | `int64` | 12 |  |
| `cache_key` | `string` | 13 | Hex digest the action probed against the content-addressable store, action_digest. Shown as the "Cache key" column on the Cacheable Actions view of the dashboard. |
| `selected_attempt` | `uint32` | 14 | Zero means no attempt executed in this run, as with retained target reuse. Otherwise this selects one attempt from the matching tuple below. |

## message `ActionAttemptStarted`

The tuple (target_execution_id, capability, action_index, attempt) is a run-local identity. An attempt begins when a worker starts preparing it; a later retry uses the next attempt number and may use another worker.

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `capability` | `string` | 2 |  |
| `action_index` | `uint32` | 3 |  |
| `attempt` | `uint32` | 4 |  |
| `worker_id` | `string` | 5 |  |

## message `ActionAttemptCompleted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `capability` | `string` | 2 |  |
| `action_index` | `uint32` | 3 |  |
| `attempt` | `uint32` | 4 |  |
| `result` | `TargetResult` | 5 |  |
| `exit_code` | `int32` | 6 |  |
| `duration_ms` | `int64` | 7 |  |
| `was_cached` | `bool` | 8 |  |

## message `TargetPhaseCompleted`

Observed target-level work, excluded from declared-action counts. Phase names are extensible and ecosystem-neutral. Unknown names are shown as opaque labels. worker_id identifies a run-local scheduling lane, not a physical host or a promise of thread affinity.

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `phase` | `string` | 2 |  |
| `worker_id` | `string` | 3 |  |
| `start_at_epoch_ms` | `int64` | 4 |  |
| `duration_ms` | `int64` | 5 |  |

## message `TestSuiteStarted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `suite_id` | `string` | 2 |  |
| `planned_case_count` | `optional uint32` | 3 |  |

## message `TestSuiteCompleted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `suite_id` | `string` | 2 |  |
| `totals` | `TestTotals` | 3 |  |
| `result_report_digest` | `ContentRef` | 4 |  |

## message `TestTotals`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `passed` | `uint32` | 1 |  |
| `failed` | `uint32` | 2 |  |
| `skipped` | `uint32` | 3 |  |
| `errored` | `uint32` | 4 |  |
| `timed_out` | `uint32` | 5 |  |
| `cancelled` | `uint32` | 6 |  |
| `flaky_final_pass` | `uint32` | 7 |  |
| `flaky_final_fail` | `uint32` | 8 |  |
| `unknown` | `uint32` | 9 |  |

## message `TestCaseStarted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `test_case_execution_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `case_id` | `string` | 3 |  |
| `name` | `string` | 4 |  |
| `class_name` | `string` | 5 |  |
| `file` | `string` | 6 |  |
| `parameters` | `string` | 7 |  |
| `tags` | `repeated string` | 8 |  |
| `attempt` | `uint32` | 9 |  |

## message `TestCaseCompleted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `test_case_execution_id` | `string` | 1 |  |
| `result` | `TestCaseResult` | 2 |  |
| `was_flaky` | `bool` | 3 |  |
| `duration_ms` | `int64` | 4 |  |
| `failure` | `TestFailure` | 5 |  |
| `case_id` | `string` | 6 | Self-describing so retrospective results do not require a fabricated start event or a server-side join to identify the case. |
| `name` | `string` | 7 |  |
| `suite_id` | `string` | 8 |  |
| `attempt` | `uint32` | 9 |  |
| `observed_duration_ms` | `optional int64` | 10 | Prefer this presence-aware measurement; the legacy duration_ms is zero when a retrospective report omits timing. |

## enum `TestCaseResult`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `TEST_CASE_RESULT_UNSPECIFIED` | value | 0 |  |
| `TEST_CASE_RESULT_PASSED` | value | 1 |  |
| `TEST_CASE_RESULT_FAILED` | value | 2 |  |
| `TEST_CASE_RESULT_SKIPPED` | value | 3 |  |
| `TEST_CASE_RESULT_TIMED_OUT` | value | 4 |  |
| `TEST_CASE_RESULT_ERRORED` | value | 5 |  |
| `TEST_CASE_RESULT_CANCELLED` | value | 6 |  |
| `TEST_CASE_RESULT_UNKNOWN` | value | 7 |  |

## message `TestCaseRetried`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `previous_test_case_execution_id` | `string` | 1 |  |
| `new_test_case_execution_id` | `string` | 2 |  |
| `new_attempt` | `uint32` | 3 |  |
| `reason` | `string` | 4 |  |

## message `TestFailure`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `message` | `string` | 1 |  |
| `expected` | `string` | 2 |  |
| `actual` | `string` | 3 |  |
| `stack_digest` | `ContentRef` | 4 |  |

## message `LogChunk`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `scope` | `LogScope` | 1 |  |
| `stream` | `Stream` | 2 |  |
| `offset` | `int64` | 3 |  |
| `bytes` | `bytes` | 4 |  |

## message `LogTruncated`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `scope` | `LogScope` | 1 |  |
| `stream` | `Stream` | 2 |  |
| `bytes_dropped` | `int64` | 3 |  |
| `since_offset` | `int64` | 4 |  |

## message `LogScope`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| *scope* | one of | | |
| `run` | `RunScope` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `test_case_execution_id` | `string` | 3 |  |

## message `RunScope`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |

## enum `Stream`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `STREAM_UNSPECIFIED` | value | 0 |  |
| `STREAM_STDOUT` | value | 1 |  |
| `STREAM_STDERR` | value | 2 |  |

## message `CacheProbe`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `cache_decision_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `action_digest` | `ContentRef` | 3 |  |
| `tier` | `string` | 4 |  |
| `tier_index` | `uint32` | 5 |  |
| `outcome` | `CacheOutcome` | 6 |  |
| `duration_ms` | `int64` | 7 |  |
| `error_class` | `string` | 8 |  |

## enum `CacheOutcome`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `CACHE_OUTCOME_UNSPECIFIED` | value | 0 |  |
| `CACHE_OUTCOME_HIT` | value | 1 |  |
| `CACHE_OUTCOME_MISS` | value | 2 |  |
| `CACHE_OUTCOME_ERROR` | value | 3 |  |
| `CACHE_OUTCOME_BYPASSED` | value | 4 |  |

## message `CacheMissReason`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `cache_decision_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `primary_reason` | `MissReasonKind` | 3 |  |
| `all_reasons` | `repeated MissReasonKind` | 4 |  |
| `analysis_status` | `MissAnalysisStatus` | 5 |  |
| `differing_inputs` | `repeated string` | 6 |  |
| `differing_inputs_total_count` | `uint32` | 7 |  |
| `differing_inputs_truncated` | `bool` | 8 |  |
| `baseline_resolution` | `BaselineResolution` | 9 |  |

## enum `MissReasonKind`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `MISS_REASON_KIND_UNSPECIFIED` | value | 0 |  |
| `MISS_REASON_KIND_FIRST_SEEN` | value | 1 |  |
| `MISS_REASON_KIND_INPUTS_CHANGED` | value | 2 |  |
| `MISS_REASON_KIND_COMMAND_CHANGED` | value | 3 |  |
| `MISS_REASON_KIND_ENV_CHANGED` | value | 4 |  |
| `MISS_REASON_KIND_TOOL_CHANGED` | value | 5 |  |
| `MISS_REASON_KIND_SALT_CHANGED` | value | 6 |  |
| `MISS_REASON_KIND_UNKNOWN` | value | 7 |  |

## enum `MissAnalysisStatus`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `MISS_ANALYSIS_STATUS_UNSPECIFIED` | value | 0 |  |
| `MISS_ANALYSIS_STATUS_COMPLETE` | value | 1 |  |
| `MISS_ANALYSIS_STATUS_PARTIAL` | value | 2 |  |
| `MISS_ANALYSIS_STATUS_TRUNCATED` | value | 3 |  |
| `MISS_ANALYSIS_STATUS_UNAVAILABLE` | value | 4 |  |

## message `BaselineResolution`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| *kind* | one of | | |
| `reference` | `BaselineReference` | 1 |  |
| `none_available` | `BaselineNoneAvailable` | 2 |  |
| `not_attempted` | `BaselineNotAttempted` | 3 |  |
| `unavailable` | `BaselineUnavailable` | 4 |  |

## message `BaselineReference`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `previous_run_id` | `string` | 1 |  |
| `previous_target_instance_id` | `string` | 2 |  |
| `previous_target_execution_id` | `string` | 3 |  |
| `previous_action_digest` | `ContentRef` | 4 |  |
| `selection_reason` | `string` | 5 |  |

## message `BaselineNoneAvailable`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |

## message `BaselineNotAttempted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `skip_reason` | `string` | 1 |  |

## message `BaselineUnavailable`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `failure_reason` | `string` | 1 |  |

## message `CacheUpload`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `cache_decision_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `content` | `ContentRef` | 3 |  |
| `tier` | `string` | 4 |  |
| `kind` | `string` | 5 |  |
| `duration_ms` | `int64` | 6 |  |
| `bytes_transferred` | `uint64` | 7 |  |

## message `CacheDownload`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `cache_decision_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `content` | `ContentRef` | 3 |  |
| `tier` | `string` | 4 |  |
| `kind` | `string` | 5 |  |
| `duration_ms` | `int64` | 6 |  |
| `bytes_transferred` | `uint64` | 7 |  |

## message `CacheStoreReused`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `cache_decision_id` | `string` | 1 |  |
| `target_execution_id` | `string` | 2 |  |
| `content` | `ContentRef` | 3 |  |
| `tier` | `string` | 4 |  |
| `kind` | `string` | 5 |  |
| `bytes_saved` | `uint64` | 6 |  |

## message `ArtifactPublished`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `kind` | `string` | 2 |  |
| `content` | `ContentRef` | 3 |  |
| `workspace_relative_path` | `string` | 4 |  |

## message `DiagnosticEmitted`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `target_execution_id` | `string` | 1 |  |
| `severity` | `Severity` | 2 |  |
| `tool` | `string` | 3 |  |
| `code` | `string` | 4 |  |
| `message` | `string` | 5 |  |
| `primary` | `Location` | 6 |  |
| `related` | `repeated Location` | 7 |  |
| `fingerprint` | `string` | 8 |  |
| `snippet` | `ContentRef` | 9 |  |

## message `Location`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `file` | `string` | 1 |  |
| `start_line` | `uint32` | 2 |  |
| `start_column` | `uint32` | 3 |  |
| `end_line` | `uint32` | 4 |  |
| `end_column` | `uint32` | 5 |  |

## enum `Severity`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `SEVERITY_UNSPECIFIED` | value | 0 |  |
| `SEVERITY_NOTE` | value | 1 |  |
| `SEVERITY_WARNING` | value | 2 |  |
| `SEVERITY_ERROR` | value | 3 |  |

## message `SystemSampled`

Periodic host-resource sample published while a run executes so the dashboard's timeline can plot CPU, memory, and network usage over wall-clock. Sampled at a bounded cadence (1 Hz by default) from a single background task on the client. The projector stores each sample as one row and derives the timeline series at read time.

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `at_epoch_ms` | `int64` | 1 | The sample's own wall-clock instant. Enveloping RunEvent.epoch_ms is for display only; RunEvent.seq defines stream order. |
| `cpu_percent` | `float` | 2 | Whole-host CPU utilisation from 0 to 100. Averaged across cores so a 4-core machine at full load reports 100, not 400. |
| `memory_bytes` | `uint64` | 3 | Whole-host memory usage in bytes, matching what a process manager would report as "used". |
| `network_in_bytes_per_second` | `uint64` | 4 | Network transfer rate at sample time, in bytes per second, summed across all interfaces. Direction is the transport's own point of view: `in` is bytes coming into the host, `out` is bytes leaving. |
| `network_out_bytes_per_second` | `uint64` | 5 |  |
| `resource_id` | `string` | 6 | A run-local identity, never a hostname or globally stable device key. |
| `scope` | `ResourceScope` | 7 |  |
| `interval_ms` | `uint32` | 8 | Actual interval used to derive rates; zero means unavailable. |

## enum `ResourceScope`

| Name | Type | Number | Description |
| --- | --- | ---: | --- |
| `RESOURCE_SCOPE_UNSPECIFIED` | value | 0 |  |
| `RESOURCE_SCOPE_HOST` | value | 1 |  |
| `RESOURCE_SCOPE_CONTAINER` | value | 2 |  |
| `RESOURCE_SCOPE_PROCESS` | value | 3 |  |
| `RESOURCE_SCOPE_WORKER` | value | 4 |  |
