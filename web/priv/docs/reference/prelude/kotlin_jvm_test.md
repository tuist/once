# `kotlin_jvm_test`

Host-side Kotlin tests for the Java virtual machine.

## Description

Compiles Kotlin test sources, scans the resulting classes for zero-argument
test methods, and emits normalized Once test results. Methods whose names begin
with `test` run without an external test framework. The runner also recognizes
the `org.junit.Test` annotation from [JUnit](https://junit.org/junit5/) and the
`kotlin.test.Test` annotation when their libraries are on the dependency
classpath.

Static nested test classes are discovered as well as top-level classes.
Public inherited test methods run on the concrete suite, with overridden
methods selected once.
Anonymous classes, local classes, abstract classes, and non-static inner
classes are not independent test suites.
Methods or suites annotated with `org.junit.Ignore` or `kotlin.test.Ignore`
are reported as skipped without invoking them or initializing ignored suites.

Set `test_framework = "junit4"` to execute annotated tests through native
JUnit 4 instead of invoking methods directly. Declare JUnit 4 and its runtime
dependencies on the target's runtime classpath. Native execution honors
inherited setup and teardown, class lifecycle, rules, expected exceptions,
assumptions, and `@RunWith` runners, including parameterized tests. Results
remain normalized per case, with assumption violations reported as skipped
and setup or teardown failures reported as failed. `Class#method` selects all
parameter values; `Class#method[0]` selects an exact parameterized case when
the runner uses that name. Aggregate suites execute their child classes once;
a class-only suite filter selects all its children. Invalid annotated methods
are reported through native JUnit initialization failures. Convention-only
methods are not JUnit tests.

## Test Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `test_framework` | string | no | `reflection` | `reflection` for convention-based tests or `junit4` for native JUnit 4 execution |
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
| Normalized results | `.once/out/<target>/test/test_results.json` |
| Test log | `.once/out/<target>/test/kotlin-jvm-test.log` |
| Native results | `.once/out/<target>/test/native_results.txt` |

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
