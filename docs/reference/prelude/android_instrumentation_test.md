# `android_instrumentation_test`

Android instrumentation test target.

## Description

Installs an Android application package under test and an Android application
package that contains instrumentation tests, then runs `am instrument` on a
connected device or emulator through Once's generic test capability.

The test package is modeled as an `android_binary` target whose `instruments`
attribute points at the app under test. The `android_instrumentation_test`
target depends on both Android application package targets. Once installs both
packages with the Android Debug Bridge (`adb`), runs instrumentation, parses
standard instrumentation status output, and writes `once.test_results.v1`.
The run succeeds only when the device reports the standard successful terminal
instrumentation code. A host command that exits successfully after a device
process crash is still reported as a failed test run.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `test_app` | target | no | inferred | Optional test package target used when more than one dep sets `instruments` |
| `android_sdk` | string | no | env | Android Software Development Kit root, otherwise `ANDROID_HOME` or `ANDROID_SDK_ROOT` |
| `adb` | string | no | Software Development Kit platform-tools | Override Android Debug Bridge path |
| `adb_serial` | string | no |  | Optional Android Debug Bridge device serial |
| `javac` | string | no | host tool | Override `javac` path used to compile the host result parser |
| `java` | string | no | host tool | Override `java` runtime path used by the host result parser |
| `java_home` | string | no | env | Override `JAVA_HOME` passed to host Java tools |
| `instrumentation_runner` | string | no | `androidx.test.runner.AndroidJUnitRunner` | Instrumentation runner class or component passed to `am instrument` |
| `instrumentation_args` | map&lt;string,string&gt; | no | `{}` | Extra `am instrument -e` arguments |
| `test_class` | string | no |  | Optional class or class#method filter lowered to `-e class` |
| `clear_package_data` | bool | no | `false` | Clear target and test package data before running instrumentation |
| `disable_animations` | bool | no | `false` | Set Android global animation scales to zero before running instrumentation |
| `env` | map&lt;string,string&gt; | no | `{}` | Bazel-compatible environment variables passed to the host instrumentation runner before `test_env` overrides |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited by the host instrumentation runner before explicit test environment values |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the host instrumentation runner |
| `args` | list&lt;string&gt; | no | `[]` | Raw arguments appended to the Android instrumentation command before its component |
| `support_apks` | list&lt;string&gt; | no | `[]` | Package-relative application package globs installed before the instrumentation application |
| `target_device` | string | no | empty | Reserved device target selection |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` for test discovery |
| `timeout_ms` | int | no |  | Optional test timeout in milliseconds |

A non-empty `target_device` fails validation because Once does not provide the
matching device provisioning behavior. Use `adb_serial` to select an already
provisioned device.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `android_apk` | The app under test and an `android_binary` test package whose `instruments` attribute points at that app |

## Providers

The target emits `android_instrumentation_test` and `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `test` | `default`, `test_results`, `logs` |

## Outputs

| Output | Location |
| --- | --- |
| Test results | `.once/out/<target>/test/test_results.json` |
| Test log | `.once/out/<target>/test/android-instrumentation-test.log` |
| Native runner output | `.once/out/<target>/test/native_results.txt` |

## Behavior

The action is not cacheable because it depends on external device state.
Once waits for a device, installs the app under test, installs the test
package, optionally clears package data, and optionally disables animations.
Files matched by `support_apks` are installed after the app under test and
before the test package. Once then runs:

```sh
adb shell am instrument -w -r <args> <test-package>/<runner>
```

`instrumentation_args` entries become repeated `-e key value` arguments.
`test_class` becomes `-e class <value>`.

After a whole-target run discovers test cases, an exact unit selected from
`once query test-manifest` is translated to the same class filter. The active
instrumentation runner must implement the standard `class` argument contract.

## Example

```toml
[[target]]
name = "GreetingApp"
kind = "android_binary"
srcs = ["src/main/**/*.kt"]

[target.attrs]
application_id = "dev.once.greeting"
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
resource_files = []

[[target]]
name = "GreetingInstrumentationApk"
kind = "android_binary"
srcs = ["src/androidTest/**/*.kt"]

[target.attrs]
application_id = "dev.once.greeting.test"
namespace = "dev.once.greeting.test"
manifest = "AndroidTestManifest.xml"
resource_files = []
instruments = "./GreetingApp"

[[target]]
name = "GreetingInstrumentationTests"
kind = "android_instrumentation_test"
deps = ["./GreetingApp", "./GreetingInstrumentationApk"]

[target.attrs]
test_app = "./GreetingInstrumentationApk"
instrumentation_runner = "dev.once.greeting.OnceInstrumentationRunner"
labels = ["device"]
```

The starter manifests declare package names and installable application
elements. Its test package includes a small self-contained instrumentation
runner, so the example does not depend on an undeclared AndroidX test runtime.
Install the Android toolchain setup before building it so the default debug
signing key is available.
