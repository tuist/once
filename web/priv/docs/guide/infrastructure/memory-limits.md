# Memory Limits

Once limits the memory pressure created by concurrent local actions. By
default, it schedules against two thirds of the memory visible to the host or
container. This leaves room for Once itself, the operating system, editors, and
other processes.

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

Cache capture, command error output, and cached output restoration stream large
files through bounded buffers and temporary files. Artifact size therefore
does not become a same-sized memory allocation during these operations.

## What The Limit Guarantees

This is an admission-control budget based on action estimates, not a hard
operating-system memory ceiling. It bounds Once's scheduling decisions and its
own artifact buffering. A command that uses more memory than its estimate can
still exceed the budget by itself.

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
