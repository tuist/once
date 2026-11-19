# `java_import`

Prebuilt Java archives.

## Description

Exposes prebuilt Java archives, such as artifacts resolved from a Maven
repository by another tool, as a `java_library` provider. Kotlin and Android
targets consume it through any dependency role that accepts `java_library`.
The archives are placed on both the compile and runtime classpaths of
consumers.

Package-relative paths and patterns are read from the workspace, and their
contents participate in consumers' action keys. Absolute host paths are
snapshotted into the build directory, so cache identity follows the archive
contents rather than its location. Struo does not download or resolve Maven
coordinates; declare each archive explicitly.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `jars` | list&lt;string&gt; | yes |  | Package-relative archive paths or patterns, or absolute host paths |

A path or pattern that matches nothing fails with the
`java_import_jar_not_found` diagnostic.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `kotlin_jvm_library`, `java_library` | Libraries the archives need, propagated to consumers' compile and runtime classpaths |
| `runtime_deps` | `kotlin_jvm_library`, `java_library` | Libraries propagated only to consumers' runtime classpaths |

## Providers

The target emits `java_library` with `jars`, `transitive_compile_jars`,
`transitive_runtime_jars`, and `transitive_data`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `jar` |

## Example

```toml
[[target]]
name = "JUnit"
kind = "java_import"

[target.attrs]
jars = ["third_party/junit-4.13.2.jar", "third_party/hamcrest-core-1.3.jar"]

[[target]]
name = "GreetingTests"
kind = "kotlin_jvm_test"
srcs = ["src/test/kotlin/**/*.kt"]
deps = ["./Greeting", "./JUnit"]

[target.attrs]
test_framework = "junit4"
```

## Limitations

Source archives, interface archive generation, and Maven coordinate resolution
are not implemented.
