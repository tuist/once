# `apple_thinned_package`

Device-specific Apple application archive.

## Description

Produces an application archive thinned for one Apple device model. The target
accepts one device build from `apple_application`, runs Xcode's thinning tool,
re-signs the resulting bundle with an ad-hoc signature, and packages it in an
archive with the `.ipa` extension.

This target is intended for services such as
[Sentry application size analysis](https://docs.sentry.io/platforms/apple/guides/ios/size-analysis/#app-thinning),
which expects the application to be thinned before upload. One target produces
variants for one device model so each model has an independent cache key.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `device_model` | string | yes |  | One Apple device model identifier, such as `iPhone17,1` (not configurable) |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_application` | Exactly one device application to thin and package |

The application must set `platform = "ios"` and `sdk_variant = "device"`.
Simulator and desktop applications fail during graph analysis.

## Providers

The target emits `apple_thinned_package`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `ipas`, `manifest` |  |

## Outputs

| Output | Location |
| --- | --- |
| Device-specific archives | `.once/out/<target>/ipas/<product>-<device_model>.ipa` |
| Package manifest | `.once/out/<target>/thinned-packages.json` |

The manifest uses the `once.apple.thinned-package.v1` schema. It records the
requested device model, every generated archive, and the installation targets
reported by Xcode without retaining Xcode's host-specific temporary paths.

## Example

```toml
[[target]]
name = "HelloDevice"
kind = "apple_application"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
sdk_variant = "device"
bundle_id = "dev.once.Hello"
minimum_os = "17.0"

[[target]]
name = "HelloSizeAnalysis"
kind = "apple_thinned_package"
deps = ["./HelloDevice"]

[target.attrs]
device_model = "iPhone17,1"
```

Build the archive with:

```sh
once build apps/Hello/HelloSizeAnalysis
```

## Limitations

The output is ad-hoc signed for size analysis. It is not a distribution-signed
archive and cannot replace the provisioning and signing flow used to install
an application on physical devices or submit it to an application store.

Xcode exposes thinning through `ipatool`, an internal interface that may change
between Xcode releases. Once validates that the requested model produced a
variant, includes the Xcode build version in the action cache identity, and
fails without emitting a universal archive when thinning produces no match.
