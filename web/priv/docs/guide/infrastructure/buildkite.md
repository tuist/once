# Buildkite

Buildkite's [dynamic pipelines](https://buildkite.com/docs/pipelines/configure/dynamic-pipelines)
accept generated JSON steps. A small adapter can select Struo triggers and
upload worker jobs without a repository pipeline YAML file.

Buildkite schedules workers and reports their status. Struo owns the build
graph inside each worker. **Generate one job per matching trigger, not one
job per Struo action.**

::: warning Example adapter, not a native integration
This example connects Buildkite to the experimental Struo worker interface.
There is no installed Struo Buildkite plugin. It covers branch-push verification
on one Linux x86-64 worker pool, not pull requests, tags, releases, or deployments.
:::

## Prepare The Service And Workers

Configure the pipeline to receive branch pushes only. Install the generator
below as `struo-buildkite-pipeline` in your trusted bootstrap image, outside
the candidate checkout. Keep pipeline settings, hooks, the generator, and
worker setup outside candidate control.

Set `STRUO_DEFINITION_REVISION` in trusted service settings to the full lowercase
commit ID containing the approved triggers. Fetch that commit in every checkout.
Do not replace this value automatically with the commit being tested. Approve
trigger changes separately, including trigger retirement.

The bootstrap needs Struo, Git, and Python 3. The `struo-linux` queue needs Struo,
Git, jq, a POSIX shell, the selected toolchains, and a clean checkout of the
build's full commit ID. Ignore `.struo/` and keep temporary event files outside
the checkout. Use separate checkouts for concurrent jobs.

**Provide an isolated execution boundary before checkout or tool setup.** Do
not expose agent tokens, pipeline-upload credentials, secrets, or other jobs'
files to project code. Agent targeting and `env` entries are not isolation;
ordinary agent defaults do not establish this boundary. The example assumes
you already enforce it with your worker infrastructure and trusted hooks.

## Generate Worker Jobs From Approved Triggers

The adapter reads activated declarations without evaluating project modules.
It matches the source ref and carries the approved definition revision and
candidate commit into each generated job. It rejects unsupported build types,
incompatible configurations, and an empty selection rather than silently
reporting a successful verification.

```python
#!/usr/bin/env python3
import json
import os
import subprocess

if (os.environ.get("BUILDKITE_SOURCE") != "webhook"
        or os.environ.get("BUILDKITE_PULL_REQUEST") != "false"
        or os.environ.get("BUILDKITE_TAG")):
    raise SystemExit("This adapter accepts branch-push builds only")

approved = os.environ["STRUO_DEFINITION_REVISION"]
subprocess.run(
    ["struo", "edit", "activate-triggers", "--revision", approved],
    check=True, stdout=subprocess.DEVNULL,
)
registered = json.loads(subprocess.check_output(
    ["struo", "--format", "json", "query", "triggers"], text=True,
))["activation"]
ref = "refs/heads/" + os.environ["BUILDKITE_BRANCH"]
revision = os.environ["BUILDKITE_COMMIT"]
steps = []

for trigger in registered["triggers"]:
    if ref not in trigger["on"]["refs"]:
        continue
    if any((i["configuration"]["os"], i["configuration"]["arch"])
           != ("linux", "x86_64") for i in trigger["invoke"]):
        raise SystemExit("This queue supports Linux x86-64 configurations only")
    steps.append({
        "key": "struo-" + trigger["id"],
        "label": "Struo: " + trigger["id"],
        "agents": {"queue": "struo-linux"},
        "timeout_in_minutes": 30,
        "env": {
            "STRUO_DEFINITION_REVISION": registered["revision"],
            "STRUO_REF": ref,
            "STRUO_REVISION": revision,
            "STRUO_TRIGGER": trigger["id"],
        },
        "command": [
            "set -eu",
            'struo edit activate-triggers --revision "$STRUO_DEFINITION_REVISION"',
            'observation=$(mktemp)',
            "trap 'rm -f \"$observation\"' EXIT",
            'jq -n --arg ref "$STRUO_REF" --arg revision "$STRUO_REVISION" '
            "'{event: \"vcs.ref_changed\", ref: $ref, revision: $revision}' "
            '> "$observation"',
            'struo --memory-limit 2GiB run --trigger "$STRUO_TRIGGER" '
            '--event "$observation"',
        ],
    })

if not steps:
    raise SystemExit("No approved Struo trigger matches this ref")
print(json.dumps({"steps": steps}))
```

Set your Buildkite pipeline's bootstrap command in its service settings to:

```sh
set -eu
pipeline=$(mktemp)
trap 'rm -f "$pipeline"' EXIT
struo-buildkite-pipeline > "$pipeline"
buildkite-agent pipeline upload --no-interpolation "$pipeline"
```

Writing the output before uploading ensures generator failure stops the
bootstrap instead of uploading partial work. `--no-interpolation` leaves
worker variables and jq expressions untouched until the worker runs. Stable
step keys make repeated uploads fail instead of adding duplicate jobs. Do
not automatically retry a bootstrap that may already have uploaded steps.

The approved trigger supplies the target selections and configurations.
The adapter supplies only service concerns: worker routing, timeout, and the
per-job memory budget. Adapt routing for additional worker pools; declaring
an OS in Struo does not provision or isolate a machine with that OS.

## Validate Before Connecting A Real Pipeline

With a representative clean checkout and Buildkite environment, inspect the
generated pipeline without changing a running build:

```sh
struo-buildkite-pipeline > /tmp/struo-pipeline.json
buildkite-agent pipeline upload --no-interpolation --dry-run --format json \
  /tmp/struo-pipeline.json
```

Dry-run validates the pipeline format, not agent placement, source trust,
required-check policy, or delivery to a real build. Test those in a disposable
Buildkite pipeline before adopting the adapter.

Struo requires the worker's clean `HEAD` to match `STRUO_REVISION`. A missing
capability, failed invocation, dirty checkout, or mismatched revision fails
the job. Publish the aggregate build result through Buildkite; do not turn a
missing job or a crashed worker into success. Retain run records from `.struo/`
through trusted infrastructure if you need them after the worker exits.

## Current Limits

Buildkite still owns event authentication, worker isolation, cancellation,
required checks, and recovery. The normalized observation is not an
authenticated event receipt. These runs use a dedicated local cache and do
not provide immutable replay. `--memory-limit` budgets work within one Struo
process; enforce OS resource limits and limits across jobs separately.

See [Event-Driven Automation](/guide/infrastructure/triggers) for the trigger
contract and integration status of other services.
