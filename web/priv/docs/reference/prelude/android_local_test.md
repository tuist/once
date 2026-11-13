# `android_local_test`

Android local test target.

## Description

Compiles Android Java and Kotlin test sources, then runs them on the host
[Java virtual machine](https://docs.oracle.com/javase/specs/jvms/se21/html/index.html)
through Once's generic test capability. This target is for local unit tests,
not device or emulator instrumentation tests.

The runner scans compiled test classes and invokes zero-argument methods whose
name starts with `test`, or methods annotated with `org.junit.Test` or
`kotlin.test.Test`. Assertion libraries such as
[JUnit](https://junit.org/) can be added through `classpath` and
`runtime_classpath`.

Static nested test classes are discovered as well as top-level classes.
Public inherited test methods run on the concrete suite, with overridden
methods selected once.
Anonymous classes, local classes, abstract classes, and non-static inner
classes are not independent test suites. Methods or suites annotated with
`org.junit.Ignore` or `kotlin.test.Ignore` are reported as skipped without
invoking them or initializing ignored suites. The reflection runner does not
implement framework lifecycle hooks or custom runners such as Robolectric.

Set `test_framework = "junit4"` to delegate execution to native JUnit 4,
including `@Before`/`@After`, rules, expected exceptions, assumptions, and
`@RunWith` runners such as Robolectric. Supply JUnit, the custom runner and all
its runtime dependencies through declared classpaths or dependency providers.
Framework configuration and SDK jars must be declared inputs too; Once does
not infer Gradle configuration or fingerprint files downloaded during a test.
For Robolectric, prepare its SDK jars before testing, declare them through
`runtime_classpath`, and use offline resolution through `jvm_flags` rather than
letting the runner download undeclared dependencies. This mode reports native
initialization failures instead of silently skipping malformed annotated tests.
Android resource tests also need the linked test resource package, its matching
runtime R classes, and the runner configuration as declared inputs. A static
library's unassigned resource IDs are not a substitute for linked runtime IDs.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `test_framework` | string | no | `reflection` | `reflection` for convention-based methods or `junit4` for native lifecycle and runner dispatch |
| `compile_sdk` | int | no | highest installed | Android Software Development Kit platform level used for `android.jar` |
| `build_tools_version` | string | no | highest installed | Android Software Development Kit build-tools version |
| `android_sdk` | string | no | env | Android Software Development Kit root, otherwise `ANDROID_HOME` or `ANDROID_SDK_ROOT` |
| `java_language_level` | string | no | `17` | Java source and target level passed to `javac` |
| `javac_opts` | list&lt;string&gt; | no | `[]` | Additional `javac` flags |
| `javacopts` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for additional `javac` flags |
| `kotlinc_opts` | list&lt;string&gt; | no | `[]` | Additional `kotlinc` flags |
| `classpath` | list&lt;string&gt; | no | `[]` | Additional Java archive files used while compiling and running tests |
| `runtime_classpath` | list&lt;string&gt; | no | `[]` | Additional Java archive files used only while running tests |
| `args` | list&lt;string&gt; | no | `[]` | Additional fully qualified class or `Class#method` filters passed to the local test runner |
| `jvm_flags` | list&lt;string&gt; | no | `[]` | Additional flags passed to the host Java virtual machine before the test classpath |
| `test_class` | string | no | empty | Optional fully qualified test class or `Class#method` filter |
| `env` | map&lt;string,string&gt; | no | `{}` | Bazel-compatible environment variables passed to the local test runner before `test_env` overrides |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited by the local test runner before explicit test environment values |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the local test runner |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` for test discovery |
| `timeout_ms` | int | no |  | Optional test timeout in milliseconds |

Accepted but unsupported attributes:
`custom_package`, `densities`, `enable_data_binding`, `manifest`,
`manifest_values`, `nocompress_extensions`, `plugins`,
`resource_configuration_filters`, `resource_jars`, `resource_strip_prefix`,
`runtime_deps`, and `stamp`. Non-empty values under `[target.attrs]` fail
validation. Use the `runtime_deps` dependency role under
`[target.dependencies]` for runtime-only libraries.

Tool override attrs are also available for `javac`, `java`, `java_home`,
`kotlinc`, `kotlin_home`, `kotlin_stdlib`, and `aapt2`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `android_library`, `java_library` | Libraries under test. `android_library` targets also emit the `java_library` provider. |
| `runtime_deps` | `android_library`, `java_library` | Libraries added only to the host test runtime classpath |

## Providers

The target emits `android_local_test` and `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `classes` |
| `test` | `default`, `test_results`, `logs` |

## Outputs

| Output | Location |
| --- | --- |
| Test results | `.once/out/<target>/test/test_results.json` |
| Test log | `.once/out/<target>/test/android-local-test.log` |
| Native runner output | `.once/out/<target>/test/native_results.txt` |

## Limitations

`android_local_test` runs on the host Java virtual machine. It does not install
an application package, start an Android device, or run instrumentation tests.
Tests that depend on Android framework behavior usually need a local framework
adapter such as Robolectric or
[`android_instrumentation_test`](/reference/prelude/android_instrumentation_test).

## Example

```toml
[[target]]
name = "Greeting"
kind = "android_library"
srcs = ["src/main/**/*.kt"]

[target.attrs]
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
resource_files = []

[[target]]
name = "GreetingTests"
kind = "android_local_test"
srcs = ["src/test/**/*.kt"]
deps = ["./Greeting"]

[target.dependencies]
runtime_deps = ["./TestRuntime"]

[target.attrs]
labels = ["unit"]
```
