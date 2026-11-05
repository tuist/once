# Memory Limits

Once limits the memory pressure created by concurrent local actions. By
default, it schedules against two thirds of the memory visible to the host or
container. This leaves room for Once itself, the operating system, editors, and
other processes.

On Linux, automatic detection considers the current process's readable cgroup
limits, including visible ancestor limits, rather than only the host total.
If cgroup information is unavailable, detection falls back to host memory.

Override the budget for any command with `--memory-limit`:

```sh
once --memory-limit 2147483648 build app
```

The value above is two gibibytes. You can also write it as `2GiB`, where `GiB`
is the abbreviation defined by the [International Electrotechnical Commission
binary-prefix standard](https://www.iec.ch/prefixes-binary-multiples).
Raw byte counts and decimal unit suffixes are accepted too.

The option is global, so these forms are equivalent:

```sh
once --memory-limit 2147483648 test app
once test app --memory-limit 2147483648
```

## How Scheduling Uses The Budget

Each local action reserves its declared memory estimate before it starts.
Actions without an estimate reserve 250 mebibytes. Once admits more work only
when the combined reservations fit within the configured budget. An action
whose estimate exceeds the entire budget runs alone instead of waiting
forever.

The same budget applies to builds, lint targets, run targets, literal commands,
annotated scripts, and test workers. Internal subprocesses inherit an explicit
limit, so nested test and agent-tool invocations do not reset to the machine
default.

When a remote cache is selected, Once carves a shared cache-transfer allowance
out of the configured memory budget. It starts at one eighth of that budget,
with a minimum of 32 MiB + 64 KiB and a maximum of 256 MiB. The minimum fits a
streamed response frame and its bounded pipe. Actions use exactly the remainder
instead of allocating a second full budget for cache requests. A finite total
must exceed that minimum; otherwise the command fails before starting work.

MCP and runtime sessions reserve the same headroom even when initially using a
local cache, because their configuration can change during the session. Ordinary
local-only commands retain the full action allowance. An explicit
`--memory-limit 0` keeps action admission unbounded while cache buffers retain
their default allowance.

Cloned cache providers share transfer admission. Scratch downloads have a separate
bounded disk budget, and oversized requests fail instead of relying on a
server-provided size to allocate arbitrarily.

Cache capture, command error output, and cached output restoration stream large
files through bounded buffers and temporary files. Artifact size therefore
does not become a same-sized memory allocation during these operations.

Graph actions, producer repairs, and legacy capabilities share the invocation's
budget. Output restoration reserves memory for each attempt and releases it before
repairing a missing producer. If an observer is cancelled or a repair times out,
blocking file work keeps its reservation and action lock until it actually stops.
This prevents newly admitted work from overlapping a reservation that was released
too early. Cache publication uses the separate carved transfer allowance after
execution finishes.

## CPU Sharing And Fairness

Set `--cpu-limit COUNT` to override the scheduler's CPU capacity:

```sh
once --cpu-limit 4 test app
```

Test batches divide the parent CPU budget instead of each inheriting the full
host capacity. This is a cooperative scheduling estimate, not a limit on threads
a tool can start. Configure tool-specific parallelism through that tool's target
kind when needed.

Host admission queues conflicting requests fairly so a stream of smaller actions
cannot indefinitely bypass an older large request. Cancelling a queued request
removes its claim without consuming capacity. Required minimum capacity is
checked separately from estimate normalization; an impossible minimum fails
instead of being silently reduced.

## What The Limit Guarantees

This is an admission-control budget based on action estimates, not a hard
operating-system memory ceiling. It bounds Once's scheduling decisions and its
own artifact buffering. A command that uses more memory than its estimate can
still exceed the budget by itself.

The budget belongs to one invocation. Independent Once processes do not share
a host-wide reservation pool, so their combined reservations can exceed a
single invocation's budget.

Use a container or operating-system process limit when a command must be
forcibly stopped at an exact resident-memory boundary. Pair that hard boundary
with a lower Once budget so the scheduler leaves enough headroom for the Once
process and the operating system.

## Choosing A Value

Start with the automatic default. Set a lower value in a small container or on
a shared machine where other services need predictable headroom. If builds
become unnecessarily serial, raise the limit gradually and declare realistic
memory estimates for unusually small or large actions.

The active value is recorded in verbose internal logs as
`memory_limit_bytes`, together with the default estimate used for actions that
do not declare one.
