def _impl(ctx):
    target = ctx["label"]["id"]
    out = _test_output_dir(ctx)
    results = out + "/test_results.json"
    filters = (ctx.get("test") or {}).get("filters") or []
    info = {
        "schema": "once.test_info.v1", "target": target,
        "runner": {"type": "fixture", "display_name": "Fixture", "metadata": {}},
        "command": {"argv": ["/bin/sh", "runner.sh"], "env": {}, "cwd": "."},
        "outputs": {"results": results, "logs": [], "native_results": [], "coverage": []},
        "listing": {"supported": True, "strategy": "normalized_results"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": True, "granularity": "file"},
        "retries": {"supported": True, "default_attempts": 1},
        "execution": {"cacheable": True, "run_from_workspace_root": True},
        "labels": [], "metadata": {},
    }
    if ctx["capability"] == "test":
        cases = [
            {"id": "native::a", "name": "recovers", "suite": "NativeSuite", "file": "one", "status": "flaky", "runner_metadata": {}, "attempts": [{"status": "failed", "duration_ms": 3, "failure": {"message": "first failure"}}, {"status": "passed", "duration_ms": 7}]},
            {"id": "native::b", "name": "passes", "suite": "NativeSuite", "file": "one", "status": "passed", "runner_metadata": {}, "attempts": [{"status": "passed", "duration_ms": 11}]},
            {"id": "native::c", "name": "skips", "suite": "OtherSuite", "file": "two", "status": "skipped", "runner_metadata": {}, "attempts": [{"status": "skipped"}]},
        ]
        cases = [case for case in cases if not filters or case["id"] in filters]
        payload = {"schema": "once.test_results.v1", "target": target, "runner": {"type": "fixture", "metadata": {}}, "status": "passed", "summary": {"total": len(cases), "passed": len([c for c in cases if c["status"] == "passed"]), "failed": 0, "flaky": len([c for c in cases if c["status"] == "flaky"]), "skipped": len([c for c in cases if c["status"] == "skipped"])}, "cases": cases, "artifacts": {"logs": [], "native_results": []}}
        template = out + "/payload.json"
        write_path(template, _json_encode(payload))
        failed_cases = [dict(case, status = "failed", attempts = [{"status": "failed", "duration_ms": 13, "failure": {"message": "persistent failure"}}]) if case["id"] == "native::a" else case for case in cases]
        failures = len([case for case in failed_cases if case["status"] == "failed"])
        failed_payload = dict(payload, status = "failed" if failures else "passed", cases = failed_cases, summary = dict(payload["summary"], failed = failures, flaky = 0))
        failed_template = out + "/failed_payload.json"
        write_path(failed_template, _json_encode(failed_payload))
        run_action(argv = ["/bin/sh", "runner.sh", results, template, ",".join(filters), failed_template], inputs = ["runner.sh", template, failed_template], outputs = [results], create_dirs = [out], identifier = "fixture")
    return {"test_info": info}

fixture = target_kind(providers = ["once_test_info"], capabilities = [capability("test", ["default", "test_results"])], impl = _impl)
