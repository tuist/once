---
prev: false
next: false
---

# Kotlin

Struo can compile Kotlin libraries, runnable main classes, and host-side tests for the
[Java virtual machine](https://docs.oracle.com/javase/specs/jvms/se21/html/index.html),
build Kotlin sources inside Android targets, and produce Kotlin/Native
frameworks for Apple consumers.

## Prerequisites

Install the pinned Kotlin compiler and
[Java Development Kit](https://docs.oracle.com/en/java/javase/21/install/overview-jdk-installation.html)
through mise:

```sh
mise install
mise exec -- kotlinc -version
mise exec -- java -version
```

## Declare a Library and Binary

```toml
[[target]]
name = "Greeting"
kind = "kotlin_jvm_library"
srcs = ["src/main/kotlin/dev/struo/greeting/**/*.kt"]

[target.attrs]
module_name = "greeting"
jvm_target = "17"

[[target]]
name = "Hello"
kind = "kotlin_jvm_binary"
srcs = ["src/main/kotlin/dev/struo/hello/**/*.kt"]
deps = ["./Greeting"]

[target.attrs]
main_class = "dev.struo.hello.MainKt"
args = ["Struo"]
```

Inspect and run the graph:

```sh
struo query schema kotlin_jvm_library
struo query target ./Hello
struo build ./Hello
struo run ./Hello
```

The library and binary compile to Java archives. The run capability builds the
archive first, then starts the main class with the complete runtime classpath.

## Mix Java Sources

List `.java` files in `srcs` next to the Kotlin sources. Kotlin and Java can
reference each other in either direction, and Java test classes in a
`kotlin_jvm_test` are discovered like Kotlin ones:

```toml
[[target]]
name = "Greeting"
kind = "kotlin_jvm_library"
srcs = ["src/main/kotlin/**/*.kt", "src/main/java/**/*.java"]

[target.attrs]
jvm_target = "1.8"
javac_opts = ["-Xlint:none"]
```

## Import Prebuilt Java Archives

Use [`java_import`](/reference/prelude/java_import) for archives built
elsewhere, such as Maven artifacts resolved by another tool:

```toml
[[target]]
name = "TestLibraries"
kind = "java_import"

[target.attrs]
jars = ["third_party/junit-4.13.2.jar", "third_party/hamcrest-core-1.3.jar"]
```

Any Kotlin or Android role that accepts `java_library` accepts the import.

## Model Classpath Roles

Use `deps` for normal compile and runtime libraries. More precise relationships
belong under `[target.dependencies]`:

```toml
[target.dependencies]
associates = ["./FriendModule"]
exported_deps = ["./PublicTypes"]
provided_deps = ["./CompileApi"]
runtime_deps = ["./RuntimeSupport"]
```

- `associates` passes Kotlin friend paths so internal declarations are visible.
- `exported_deps` also propagates to downstream compile and runtime classpaths.
- `provided_deps` appears only while compiling.
- `runtime_deps` appears only while running.

Struo validates every role against its own provider contract. This catches a
runtime artifact in a compiler-only role before invoking Kotlin.

## Use Compiler Plug-ins

Pass package-relative compiler plug-in Java archives and their options through
attributes:

```toml
[target.attrs]
compiler_plugin_jars = ["tools/serialization-plugin.jar"]
compiler_plugin_options = [
  "plugin:org.example.serialization:enabled=true",
]
```

The archive contents and options participate in the action key. Generated
source directories and Kotlin symbol processing are not implemented yet, so
keep those pipelines in a script target until their outputs can be represented
as normal graph artifacts.

## Run Host-side Tests

Declare `kotlin_jvm_test` with normal compile dependencies and optional
runtime-only dependencies:

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
```

Run the target with `struo test GreetingTests`. The runner discovers
zero-argument methods whose names begin with `test`, and it also recognizes
[JUnit](https://junit.org/junit5/) and Kotlin test annotations when those libraries are present on the
classpath. Struo writes the standard `struo.test_results.v1` record, so test
discovery, affected-test queries, and Model Context Protocol tools see the same
shape as other ecosystems.

## Android and Apple

Use [`android_library`](/reference/prelude/android_library) or
[`android_binary`](/reference/prelude/android_binary) for Android Kotlin
sources because those target kinds also own resources and application
packaging. Use
[`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework) when an
Apple target should consume Kotlin/Native code as a framework.

## Supported Target Kinds

- [`kotlin_jvm_library`](/reference/prelude/kotlin_jvm_library)
- [`kotlin_jvm_binary`](/reference/prelude/kotlin_jvm_binary)
- [`kotlin_jvm_test`](/reference/prelude/kotlin_jvm_test)
- [`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework)

Resource packaging, annotation processing, Kotlin symbol processing, incremental compilation, interface archive generation,
and test framework lifecycle integration remain outside the typed Kotlin Java
virtual machine surface. Android local tests remain available through
[`android_local_test`](/reference/prelude/android_local_test).
