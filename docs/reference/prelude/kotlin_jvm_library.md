# `kotlin_jvm_library`

Kotlin library for the Java virtual machine.

## Description

Compiles Kotlin sources with `kotlinc` into one Java archive. The provider can
feed other Kotlin targets and Android targets that accept `java_library`.
Dependency roles distinguish normal, friend, exported, compile-only, and
runtime-only classpaths.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `module_name` | string | no | target name | Kotlin module name |
| `output` | string | no | `<target>.jar` | Output Java archive file name |
| `jvm_target` | string | no | `17` | Java virtual machine bytecode target |
| `language_version` | string | no | compiler default | Kotlin language version |
| `api_version` | string | no | compiler default | Kotlin application programming interface version |
| `kotlinc_opts` | list&lt;string&gt; | no | `[]` | Additional Kotlin compiler arguments |
| `compiler_plugin_jars` | list&lt;string&gt; | no | `[]` | Package-relative Kotlin compiler plug-in Java archives |
| `compiler_plugin_options` | list&lt;string&gt; | no | `[]` | Kotlin compiler plug-in options passed through `-P` |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data patterns propagated to binaries |
| `kotlinc` | string | no | host tool | Override the Kotlin compiler path |
| `kotlin_home` | string | no | inferred | Kotlin installation root used to find the standard library |
| `kotlin_stdlib` | string | no | inferred | Override the Kotlin standard library Java archive path |
| `java_home` | string | no | `JAVA_HOME` | Java runtime home exposed to the compiler |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `kotlin_jvm_library`, `java_library` | Libraries used for compilation and runtime |
| `associates` | `kotlin_jvm_library` | Friend modules whose internal declarations are visible during compilation |
| `exported_deps` | `kotlin_jvm_library`, `java_library` | Libraries exported to downstream compile and runtime classpaths |
| `provided_deps` | `kotlin_jvm_library`, `java_library` | Libraries available for compilation but omitted from this target's runtime classpath |
| `runtime_deps` | `kotlin_jvm_library`, `java_library` | Libraries added only to this target's runtime classpath |

Declare named roles under `[target.dependencies]`.

## Providers

The target emits `kotlin_jvm_library` and `java_library`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `compile_jar` | string | Java archive used on compile classpaths |
| `runtime_jar` | string | Java archive used on runtime classpaths |
| `transitive_compile_jars` | list&lt;string&gt; | Compile classpath propagated to consumers |
| `transitive_runtime_jars` | list&lt;string&gt; | Runtime classpath propagated to consumers |
| `transitive_data` | list&lt;string&gt; | Runtime data propagated to binaries |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `jar` |

## Outputs

| Output | Location |
| --- | --- |
| Java archive | `.once/out/<target>/<output>` |

## Example

```toml
[[target]]
name = "Greeting"
kind = "kotlin_jvm_library"
srcs = ["src/main/kotlin/**/*.kt"]

[target.dependencies]
provided_deps = ["./CompileApi"]
runtime_deps = ["./RuntimeSupport"]

[target.attrs]
module_name = "greeting"
jvm_target = "17"
```

## Limitations

Java source compilation, resource packaging, annotation processing, Kotlin
symbol processing, incremental compilation, and source-only interface archive
generation are not implemented yet. Use an Android target for mixed Android
Java and Kotlin sources, or keep these steps behind a script target.
