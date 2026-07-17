use super::*;

#[test]
fn attempt_identity_changes_with_physical_attempt() {
    let first = attempt(10, "local-1");
    let second = attempt(11, "local-1");

    assert_ne!(first.id, second.id);
    assert_eq!(first.batch_id, second.batch_id);
}

#[test]
fn schedule_orders_attempts_by_start_time() {
    let schedule = TestSchedule::new(
        "plan",
        "dynamic",
        2,
        10,
        30,
        20,
        vec![attempt(20, "local-2"), attempt(10, "local-1")],
    )
    .unwrap();

    assert_eq!(schedule.attempts[0].worker, "local-1");
    assert_eq!(schedule.attempts[1].worker, "local-2");
}

fn attempt(started_at_unix_ms: i64, worker: &str) -> TestBatchAttempt {
    TestBatchAttempt::new(TestBatchAttemptSpec {
        id: format!("attempt-{started_at_unix_ms}"),
        plan_id: "plan".to_string(),
        batch_id: "batch".to_string(),
        target: "tests/unit".to_string(),
        attempt: 1,
        placement: "local".to_string(),
        worker: worker.to_string(),
        estimated_duration_ms: None,
        started_at_unix_ms,
        finished_at_unix_ms: started_at_unix_ms + 5,
        duration_ms: 5,
        status: TestBatchStatus::Passed,
        exit_code: Some(0),
        cache: Some("miss".to_string()),
    })
    .unwrap()
}
