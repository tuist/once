# `kotlin_jvm_test`

Host-side Kotlin tests for the Java virtual machine.

## Description

Compiles Kotlin test sources, scans the resulting classes for zero-argument
test methods, and emits normalized Once test results. Methods whose names begin
with `test` run without an external test framework. The runner also recognizes
the `org.junit.Test` annotation from [JUnit](https://junit.org/junit5/) and the
`kotlin.test.Test` annotation when their libraries are on the dependency
classpath.

## Test Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `test_class` | string | no |  | Fully qualified class or `Class#method` filter |
| `args` | list&lt;string&gt; | no | `[]` | Additional class or `Class#method` filters |
| `jvm_flags` | list&lt;string&gt; | no | `[]` | Flags passed to the Java virtual machine before the test classpath |
| `env` | map&lt;string,string&gt; | no | `{}` | Environment variables applied before `test_env` |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names to inherit |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables applied last |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed during test discovery |
| `timeout_ms` | integer | no |  | Optional test timeout in milliseconds |
| `java` | string | no | host tool | Override the Java runtime path |
| `javac` | string | no | host tool | Override the Java compiler used for the reflection runner |

The compiler attributes from
[`kotlin_jvm_library`](/reference/prelude/kotlin_jvm_library#attributes) also
apply.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `kotlin_jvm_library`, `java_library` | Libraries used for compilation and runtime |
| `associates` | `kotlin_jvm_library` | Friend modules whose internal declarations are visible during compilation |
| `exported_deps` | `kotlin_jvm_library`, `java_library` | Libraries propagated to compile and runtime classpaths |
| `provided_deps` | `kotlin_jvm_library`, `java_library` | Libraries available only while compiling |
| `runtime_deps` | `kotlin_jvm_library`, `java_library` | Libraries available only while running tests |

## Providers

The target emits `kotlin_jvm_test` and `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `test` | `default`, `test_results`, `logs` |

## Outputs

| Output | Location |
| --- | --- |
| Normalized results | `.once/out/<target>/test-results/test_results.json` |
| Test log | `.once/out/<target>/test-results/kotlin-jvm-test.log` |
| Native results | `.once/out/<target>/test-results/native_results.txt` |

## Example

```toml
[[target]]
name = "GreetingTests"
kind = "kotlin_jvm_test"
srcs = ["src/test/kotlin/**/*.kt"]
deps = ["./Greeting"]

[target.dependencies]
runtime_deps = ["./TestRuntime"]

[target.attrs]
labels = ["unit"]
test_class = "dev.once.greeting.GreetingTest"
```

Run all selected methods with `once test GreetingTests`. Pass a
`Class#method` value through `test_class` or `args` for a narrower run.

## Limitations

The reflection runner does not implement test framework lifecycle hooks,
parameterized tests, sharding, retries, or coverage collection. Frameworks can
still supply assertions and annotations through normal dependencies.
