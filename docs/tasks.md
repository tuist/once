# Tasks

Fabrik supports checked-in runtime tasks through `[[task]]` targets. Run them with `fabrik run`.

```toml
[[task]]
name = "print"
argv = ["/bin/sh", "-c", "cat tasks/input.txt"]
srcs = ["input.txt"]
```

```sh
fabrik run //tasks:print
```

## Fields

- `name`: target name.
- `argv`: command and arguments.
- `srcs`: package-relative inputs that participate in the cache key.
- `src_globs`: package-relative glob inputs.
- `outputs`: workspace-relative outputs to capture and restore from CAS.
- `env`: environment variables passed to the process.
- `cwd`: workspace-relative working directory.
- `cache`: defaults to `true`. Set to `false` for side-effecting tasks.
- `timeout_ms`: optional timeout.
- `cpu_slots`: optional CPU slot request. Defaults to 1.
- `memory_bytes`: optional memory request. Defaults to 0, which means no memory budget request.

Environment values use a TOML subtable:

```toml
[[task]]
name = "with_env"
argv = ["/bin/sh", "-c", "printf \"$MESSAGE\""]

[task.env]
MESSAGE = "hello"
```

## Cache Behavior

Tasks are cacheable by default. The cache key includes argv, env, cwd, declared inputs, declared outputs, timeout, and resource request. A cache hit replays stdout, stderr, exit code, and restores declared outputs.

Use `cache = false` for tasks with runtime side effects:

```toml
[[task]]
name = "counter"
argv = ["/bin/sh", "-c", "printf run >> tasks/log"]
cache = false
```

Uncached tasks still run through the same bounded executor, but every invocation gets a fresh action key.
