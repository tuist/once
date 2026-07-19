# `kotlin_jvm_binary`

Runnable Kotlin target for the Java virtual machine.

## Description

Compiles Kotlin sources into a Java archive and runs a required main class
through the host Java runtime. It uses the same dependency roles and compiler
attributes as [`kotlin_jvm_library`](/reference/prelude/kotlin_jvm_library).

## Binary Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `main_class` | string | yes |  | Fully qualified Kotlin main class, such as `dev.once.hello.MainKt` |
| `args` | list&lt;string&gt; | no | `[]` | Arguments passed to the main class |
| `jvm_flags` | list&lt;string&gt; | no | `[]` | Flags passed to the Java virtual machine before the classpath |
| `run_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the Java runtime |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited before `run_env` overrides |
| `java` | string | no | host tool | Override the Java runtime path |

The common attributes from
[`kotlin_jvm_library`](/reference/prelude/kotlin_jvm_library#attributes) also
apply.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `kotlin_jvm_library`, `java_library` | Libraries used for compilation and runtime |
| `associates` | `kotlin_jvm_library` | Friend modules whose internal declarations are visible during compilation |
| `exported_deps` | `kotlin_jvm_library`, `java_library` | Libraries exported to downstream compile and runtime classpaths |
| `provided_deps` | `kotlin_jvm_library`, `java_library` | Libraries available for compilation but omitted from runtime |
| `runtime_deps` | `kotlin_jvm_library`, `java_library` | Libraries added only to runtime |

## Providers

The target emits `kotlin_jvm_binary`.

## Capabilities

| Capability | Output groups | Required output groups |
| --- | --- | --- |
| `build` | `default`, `jar` | none |
| `run` | `default` | `jar` |

## Outputs

| Output | Location |
| --- | --- |
| Java archive | `.once/out/<target>/<output>` |
| Run log | `.once/out/<target>/run/stdout.log` |
| Run result | `.once/out/<target>/run/run.json` |

## Example

```toml
[[target]]
name = "Hello"
kind = "kotlin_jvm_binary"
srcs = ["src/main/kotlin/**/*.kt"]
deps = ["./Greeting"]

[target.dependencies]
runtime_deps = ["./RuntimeSupport"]

[target.attrs]
main_class = "dev.once.hello.MainKt"
args = ["Once"]
```

## Limitations

The target runs a main class on the host. It does not produce a self-contained
distribution, shaded Java archive, native launcher, or platform installer.
