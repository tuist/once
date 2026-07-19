---
prev: false
next: false
---

# C Software Development Kit

The C library is the lowest-level supported language binding. Link `libonce`
and include `once.h` when an application needs direct cache access without a
language wrapper. Script and graph execution remain command-line features.

Build or obtain the shared or static `libonce` library, then use `once.h` from
the matching tagged source distribution. Add both to the host build system.

## Store A First Blob

This complete example stores `hello` in the default local cache and prints the
response:

```c
#include <once.h>
#include <stdio.h>

int main(void) {
    const char *request = "{\"bytes\":[104,101,108,108,111]}";
    char *response = once_cache_put_blob_json(request);

    puts(response);
    once_string_free(response);
    return 0;
}
```

## Ownership And Responses

Every `once_*` function that returns `char *` returns an owned
[Unicode Transformation Format, 8-bit](https://www.unicode.org/faq/utf_bom.html#UTF8)
string. Release it with `once_string_free`, including error responses.

Cache and action-key requests use
[JavaScript Object Notation](https://www.json.org/json-en.html). Responses have
one of these shapes:

```json
{ "status": "ok", "value": "..." }
```

```json
{ "status": "error", "message": "..." }
```

## Cache Selection

Omit cache-selection fields to use the default local cache. Add exactly one of
these optional fields to any cache request:

| Field | Use |
| --- | --- |
| `local_cache_root` | Opens an isolated local cache at this path. |
| `workspace_root` | Resolves the same effective provider as the command line for this workspace. |

The workspace selection follows the provider precedence described in the
[language-library overview](/guide/sdk/).

## Blobs

| Function | Use |
| --- | --- |
| `once_digest_bytes` | Hashes a byte buffer without touching storage. |
| `once_cache_put_blob_json` | Stores the `bytes` array from a request. |
| `once_cache_put_file_json` | Stores the file at `path` without structured-text byte expansion. |
| `once_cache_get_blob_json` | Returns a blob as a `bytes` array. |
| `once_cache_get_blob_to_file_json` | Writes a blob to `path` and returns its byte count. |
| `once_cache_has_blob_json` | Returns whether a digest exists. |

Use the file functions for payloads whose size is not tightly bounded.

## Action Keys And Results

`once_action_key_json` accepts a namespace and ordered inputs:

```json
{
  "namespace": "example.compile",
  "inputs": [
    { "kind": "bytes", "label": "compiler", "bytes": [115, 119, 105, 102, 116, 99] },
    { "kind": "digest", "label": "source", "digest": "<64 lowercase hexadecimal characters>" }
  ]
}
```

Input order is significant and must be deterministic. The returned digest can
be passed to the action-result functions:

| Function | Use |
| --- | --- |
| `once_cache_put_action_result_json` | Stores action metadata under `action_digest`. |
| `once_cache_get_action_result_json` | Returns action metadata when it exists. |
| `once_cache_forget_action_json` | Removes action metadata without deleting referenced blobs. |

## Statistics

`once_cache_stats_json` reports statistics for the local cache tier. It takes
no action digest and describes the local tier only.
