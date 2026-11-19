# `kotlin_jvm_library`

Kotlin library for the Java virtual machine.

## Description

Compiles Kotlin sources with `kotlinc` into one Java archive. The provider can
feed other Kotlin targets and Android targets that accept `java_library`.

Java sources (`.java`) matched by `srcs` compile jointly with the Kotlin
sources. `kotlinc` reads the Java sources so Kotlin can reference Java
declarations, then `javac` compiles the Java sources against the Kotlin
classes, so Java can reference Kotlin declarations too. Both class trees are
merged into the archive with fixed entry timestamps and permissions, so the
archive bytes depend only on the class files. `javac` receives
`--release` derived from `jvm_target` (`1.8` becomes `8`) unless `javac_opts`
sets `--release`, `-source`, or `-target`.
Dependency roles distinguish normal, friend, exported, compile-only, and
runtime-only classpaths.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `module_name` | string | no | target name | Kotlin module name |
| `output` | string | no | `<target>.jar` | Output Java archive file name |
| `manifest` | string | no | none | Package-relative JAR manifest input |
| `jar` | string | no | resolved JDK tool | Override the JDK archive tool used with `manifest` |
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
| `javac` | string | no | host tool | Override the Java compiler used for Java sources |
| `javac_opts` | list&lt;string&gt; | no | `[]` | Additional Java compiler arguments for Java sources |

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
| Java archive | `.struo/out/<target>/<output>` |
| Kotlin classes, with Java sources | `.struo/out/<target>/kotlin_classes` |
| Java classes, with Java sources | `.struo/out/<target>/java_classes` |

With `manifest`, compilation emits a declared classes tree and a separate
archive action packages it with the supplied manifest using JDK 17 or newer
`jar`. Archive entry timestamps are fixed, and the manifest contents and archive
tool identity participate in caching. Without `manifest` or Java sources,
`kotlinc` continues to emit the archive directly. Struo does not infer publication metadata or
invent library versions.

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

Resource packaging, Java annotation processing, Kotlin symbol processing,
incremental compilation, and source-only interface archive generation are not
implemented yet. Keep these steps behind a script target. Use
[`java_import`](/reference/prelude/java_import) for prebuilt archives.
