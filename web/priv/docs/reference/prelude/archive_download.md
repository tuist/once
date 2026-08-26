# `archive_download`

Checksum-pinned archive download.

## Description

Downloads a [ZIP archive](https://www.loc.gov/preservation/digital/formats/fdd/fdd000354.shtml),
checks its [Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
digest, and materializes its contents as a cacheable directory. The source URL
and checksum identify the action, so a cache hit restores the extracted
directory without another download.

Use `authorization_env` only when the archive server requires authentication.
It names an environment variable that provides a web Authorization header
while the action runs. Its value is never stored in the graph, action cache,
logs, or build evidence.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `url` | string | yes |  | Web archive URL |
| `sha256` | string | yes |  | Expected 64-character Secure Hash Algorithm 256-bit digest |
| `authorization_env` | string | no |  | Optional environment-variable name that supplies the web Authorization header while downloading |

None of these attributes are configurable by platform select.

## Providers and capabilities

The target emits `artifact` and exposes `build` with `default` and `artifact`
output groups. Consumers such as `apple_xcframework_import` can depend on the
artifact so it is materialized before their analysis runs.

## Limitations

Only web-delivered ZIP archives are supported. The archive is verified before
extraction and entries that escape the declared output directory are rejected.

## Example

```toml
[[target]]
name = "VendorArchive"
kind = "archive_download"

[target.attrs]
url = "https://example.com/Vendor.zip"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```
