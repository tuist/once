---
prev: false
next: false
---

# Apple

Struo can build libraries, frameworks, applications, and test bundles for
Apple platforms. This guide starts with one iOS application and a library it
depends on, then queries, builds, runs, and tests that same project.

If you already have an Xcode project or workspace, you do not need to declare
these targets by hand. [Xcode Projects](/guide/graph/apple/xcode) shows how to point
Struo at an existing `.xcodeproj` or `.xcworkspace` and get the same targets
derived for you.

## Prerequisites

On macOS, Apple targets use the developer tools selected by `xcrun`, `swiftc`,
and `clang`. Verify that the Swift compiler is available:

```sh
xcrun --find swiftc
```

Running this guide's iOS application also requires an installed Simulator
runtime. Building a library does not require a running simulator.

Linux can cross-compile native Apple targets using Swift, xtool's Darwin SDK,
and Linux resource and signing tools. See [Build iOS Applications on
Linux](/guide/graph/apple/linux) for installation, a runnable starter, and the
boundaries between building, provisioning, and device deployment.

## Explicit Modules and Dependency Checks

Set `explicit_modules = true` in an Apple target's attributes to turn imported
Swift interfaces and Clang modules into separate cacheable build actions.
Struo first runs the Swift compiler's dependency scanner, builds the discovered
modules, then compiles the target with implicit module building disabled.
Independent module actions share the command's memory budget, and identical
module actions can reuse results across targets.
Scans use separate, cleared scratch directories. Module output paths include
their compiler commands and input identities, so different modules cannot
overwrite one another merely because their names match.

```toml
[[target]]
name = "Core"
kind = "apple_library"
srcs = ["Sources/Core/**/*.swift"]
deps = ["Models"]

[target.attrs]
platform = "macos"
explicit_modules = true
dependency_check = "error"
```

The same attributes are available on Swift macros, frameworks, applications,
executables, and test bundles. Set them on a `swift_package_workspace` or
`xcode_workspace` seed to propagate them to resolved Swift targets. Native
Xcode targets also honor `SWIFT_ENABLE_EXPLICIT_MODULES = YES`.

`dependency_check` defaults to `"off"`. With `"error"`, source imports of
workspace modules must name direct dependencies, even when a transitive or
inferred dependency makes the module available. System modules are exempt.
An error identifies the consuming target, the missing module, and a suggested
manifest repair. Checking requires explicit modules and a Swift compiler whose
scan reports source-import information; unsupported compilers fail clearly.

Compiler binaries, development-kit contents, Clang resources, and host Swift
plugins contribute to module cache identity. Changing a declared header or
interface invalidates the corresponding module action. Existing projects keep
implicit modules by default.

This is not a downloadable hermetic Apple toolchain. Builds still require the
selected Xcode installation on macOS or the installed Darwin SDK on Linux,
and cached scanner commands retain their host paths.
Module inputs outside the workspace and the identified development-kit and
toolchain directories are rejected instead of being silently omitted from
cache identity. Move such dependencies into the workspace or use implicit
modules for those targets.
Compiler scans that require auxiliary files created only inside their temporary
cache are also rejected with a repair diagnostic. Keep interfaces and headers
in the workspace, or leave explicit modules disabled for those targets.
The scheduler currently reserves the default 250 mebibytes per module action.
This is an admission estimate, not a hard limit on compiler memory use; large
module builds can need more memory.
Standalone C-family compilation is unchanged. Initial graph queries show the
scan and deferred planning step; execution evidence records the module actions
discovered at build time. Target scheduling still waits for dependency targets
to complete, rather than releasing consumers as soon as one module is ready.

## Declare the Application

Create `apps/Hello/struo.toml` with a reusable library and an application that
depends on it:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/AppCore/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"

[[target]]
name = "Hello"
kind = "apple_application"
srcs = ["Sources/HelloApp/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
bundle_id = "dev.struo.Hello"
minimum_os = "17.0"
families = ["iphone"]
```

`./AppCore` resolves in the same package. The dependency is typed:
`apple_application` accepts the linkable output produced by
`apple_library`.

The smallest useful source layout is:

```text
apps/Hello/
├── struo.toml
└── Sources/
    ├── AppCore/
    └── HelloApp/
```

The application source needs an entry point, such as a Swift type marked with
`@main`. The library contains code imported by the application.

## Query Before Building

Inspect the exact targets declared above:

```sh
struo query targets
struo query capabilities apps/Hello/AppCore
struo query capabilities apps/Hello/Hello
struo query schema apple_application
```

`AppCore` exposes `build`. `Hello` exposes `build` and `run`. The schema query
shows the application attributes, accepted dependencies, capabilities, and
outputs without starting a compiler.

## Build and Run

Build the application bundle:

```sh
struo build apps/Hello/Hello
```

Struo builds `AppCore` first because `Hello` depends on it. Outputs are
materialized under `.struo/out/<target>/`; the
[`apple_application` reference](/reference/prelude/apple_application) lists
the exact output groups.

Launch the application in an iOS simulator:

```sh
struo run --visible apps/Hello/Hello
```

Struo selects or boots a simulator, installs the application bundle, and
launches its bundle identifier. The launch runs on every invocation instead
of replaying from the action cache. Omit `--visible` when Simulator does not
need to be brought to the foreground.

macOS applications use the host application launcher. Launching directly on
an iPhone or iPad is not supported yet.

## Create a Device-Specific Size-Analysis Archive

Size-analysis services can report confusing results when they receive a
universal application. Sentry, for example, does not thin an uploaded
application and [recommends thinning it before
upload](https://docs.sentry.io/platforms/apple/guides/ios/size-analysis/#app-thinning).

Add a device application and a package target for the device model you want to
measure:

```toml
[[target]]
name = "HelloDevice"
kind = "apple_application"
srcs = ["Sources/HelloApp/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
sdk_variant = "device"
bundle_id = "dev.struo.Hello"
minimum_os = "17.0"
families = ["iphone"]

[[target]]
name = "HelloSizeAnalysis"
kind = "apple_thinned_package"
deps = ["./HelloDevice"]

[target.attrs]
device_model = "iPhone17,1"
```

Build the device-specific archive:

```sh
struo build apps/Hello/HelloSizeAnalysis
```

The target writes one or more archives with the `.ipa` extension and a stable
manifest that maps them to the requested device model. Declare another
`apple_thinned_package` target for each additional model so builds and cache
entries stay independent.

These archives use ad-hoc signing and are intended for size analysis. They do
not replace distribution signing or provisioning for physical-device
installation. See the
[`apple_thinned_package` reference](/reference/prelude/apple_thinned_package)
for the exact outputs and validation rules.

## Add a Test Target

Add a test target to the same `apps/Hello/struo.toml`:

```toml
[[target]]
name = "AppCoreTests"
kind = "apple_test_bundle"
srcs = ["Tests/AppCoreTests/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
swift_testing = true
labels = ["unit"]
```

Place tests that use Swift Testing under `Tests/AppCoreTests/`, then inspect
and run the new capability:

```sh
struo query capabilities apps/Hello/AppCoreTests
struo test apps/Hello/AppCoreTests
```

Apple tests currently support macOS logic tests and iOS simulator bundles.
An application host discovered through dependency providers is supported,
while direct `test_host` attributes, custom destinations, test plans, and
device runners are not implemented. Non-empty unsupported attributes fail
during graph analysis instead of being ignored. See the
[`apple_test_bundle` reference](/reference/prelude/apple_test_bundle) before
adding those features.

## Choose Values by Configuration

Configurable attributes can use `select`. For example, a library shared by an
iOS and macOS target can choose the platform framework it links:

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

Apple configuration keys include platform names, architectures, `simulator`,
`device`, `mac_catalyst`, combined keys such as `ios:simulator`, and
`default`. When more than one branch matches, the most specific branch wins.

Attributes that determine the configuration, including `platform`,
`sdk_variant`, `archs`, and `mac_catalyst`, must remain literal. The target
kind schema identifies any other non-configurable attributes.

See the [configurations guide](/guide/graph/configuration) for how
`--config` overrides these tokens per invocation and how the effective
configuration scopes outputs.

## Connect Native Dependencies

Apple targets can consume native outputs from other ecosystems through normal
`deps` entries:

- [`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework)
  produces a framework that an application or test can link and embed.
- [`rust_mobile_library`](/reference/prelude/rust_mobile_library) produces an
  Apple static library for an Apple consumer.

Add these only after the application builds on its own, then query the
consumer again to confirm that the dependency contract is satisfied.

## macOS Applications and Distribution

Materialize the macOS starter in an empty directory:

```sh
struo edit materialize-example apple_application apple-application-macos
struo build Hello
struo run Hello
```

The starter loads a text resource through `Bundle.main`. macOS applications
use the native `Contents/MacOS`, `Contents/Resources`, and `Contents/Frameworks`
layout. Framework dependencies automatically receive the
`@executable_path/../Frameworks` runpath. Declare system frameworks with
`sdk_frameworks`, not custom linker or SDK search-path scripts.

Existing scripts that assumed a flat macOS bundle must use the new paths.
Native Xcode script phases receive the corresponding `EXECUTABLE_PATH`,
`INFOPLIST_PATH`, and `UNLOCALIZED_RESOURCES_FOLDER_PATH` settings. iOS bundle
paths are unchanged. When an existing generated macOS bundle has a root
`Info.plist`, Struo removes that legacy product before rebuilding so obsolete
flat files do not invalidate the new bundle's signature.

Versioned prebuilt frameworks retain their symlinks and nested code. Struo
copies and ad-hoc signs the embedded copy, without changing the imported
framework. Do not add a root `_CodeSignature` directory or signature-forwarding
symlink: signing captures the complete framework tree, including the version's
actual signature. `apple_xcframework_import` selects a matching slice from an
XCFramework. Other archive layouts still require a project module; the built-in
`archive_download` currently accepts ZIP, not tar archives.

### Keep Release Credentials Outside Cached Builds

`struo build` produces an ad-hoc signed bundle for local use. Developer ID
signing and notarization are separate release steps, not `postbuild_actions`,
annotated `struo exec` scripts, or remotely cached target actions. Keep private
keys, keychain passwords, provisioning profiles, and notary credentials outside
the workspace and Struo's declared inputs, environment, outputs, and logs.

Build first without release credentials. In a separate release job, copy the
bundle into a private temporary directory. Import the authorized Developer ID
Application certificate into an isolated keychain and store notarization
credentials in a `notarytool` keychain profile. Avoid changing the machine's
default keychain or search list. Disable shell tracing, delete temporary
keychains on failure as well as success, and never publish the temporary
directory as a build artifact.

For the starter, which has no nested code or restricted entitlements, the
following release script signs and notarizes only its private copy. Supply
`DEVELOPER_ID_SHA1` as the selected certificate fingerprint, `SIGNING_KEYCHAIN`
as the isolated keychain path, and `NOTARY_PROFILE` as its credential profile
name. These selectors are not passwords. The final ZIP is written to a new
`dist` directory, after notarization and local policy checks succeed:

```sh
#!/bin/sh
set -eu
umask 077
: "${DEVELOPER_ID_SHA1:?Select a Developer ID Application certificate}"
: "${SIGNING_KEYCHAIN:?Provide an isolated signing keychain}"
: "${NOTARY_PROFILE:?Provide a notarytool credential profile}"
test ! -e dist
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
app="$work/Hello.app"
ditto .struo/out/Hello/Hello.app "$app"
codesign --force --sign "$DEVELOPER_ID_SHA1" --keychain "$SIGNING_KEYCHAIN" \
  --options runtime --timestamp "$app"
codesign --verify --deep --strict "$app"
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/submission.zip"
xcrun notarytool submit "$work/submission.zip" --wait \
  --keychain "$SIGNING_KEYCHAIN" --keychain-profile "$NOTARY_PROFILE" \
  --output-format json > "$work/notarization.json"
test "$(plutil -extract status raw -o - "$work/notarization.json")" = Accepted
xcrun stapler staple "$app"
xcrun stapler validate "$app"
codesign --verify --deep --strict "$app"
spctl --assess --type execute "$app"
mkdir dist
ditto -c -k --sequesterRsrc --keepParent "$app" dist/Hello-macos.zip
```

Run this as an ordinary shell script, outside Struo. Its caller owns keychain
creation and cleanup. Never sign, change a property list, or staple the bundle
under `.struo/out` in place. Create update signatures and checksums from the
final stapled ZIP, not the submission archive.

For applications with nested helpers, frameworks, XPC services, or extensions,
sign each nested executable and bundle inside out before the enclosing
application. Use application-specific entitlements for each component, and do
not use `codesign --deep` to sign. Apps with restricted capabilities such as
push notifications must validate the profile's team, application identifier,
expiration, signing certificate, and authorized entitlements before signing.
Do not strip these entitlements or replace them with ad-hoc signing to make
validation pass. The minimal starter script does not implement provisioning or
nested-code signing. See Apple's [code signing guidance](https://developer.apple.com/library/archive/technotes/tn2206/_index.html)
and [notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
for distribution requirements.

## Supported Target Kinds and Limitations

Use the target kind reference for the contract that matches the artifact:

- [`apple_library`](/reference/prelude/apple_library)
- [`apple_framework`](/reference/prelude/apple_framework)
- [`apple_application`](/reference/prelude/apple_application)
- [`apple_resource_bundle`](/reference/prelude/apple_resource_bundle)
- [`apple_thinned_package`](/reference/prelude/apple_thinned_package)
- [`apple_test_bundle`](/reference/prelude/apple_test_bundle)
- [`apple_xcframework_import`](/reference/prelude/apple_xcframework_import)
- [`swift_macro`](/reference/prelude/swift_macro)

Provisioning profiles, signing identities, and non-ad-hoc signing are accepted
by the schema but are not supported yet. Using a non-empty value for one of
these attributes fails validation before the build starts.

## Next

Continue with [Memory](/guide/memory/) once the application builds and tests.
It shows how Struo records durable context about graph work. If the application
also contains Android or Rust code, use [Ecosystems](/guide/graph/ecosystems)
to choose the next independent integration.
