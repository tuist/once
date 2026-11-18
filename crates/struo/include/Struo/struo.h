#ifndef STRUO_H
#define STRUO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// All functions are thread-safe and reentrant. The first cache call lazily
// initializes a shared runtime; concurrent first calls are safe and require
// no caller-side ordering.
//
// Returned char pointers are owned by Struo and must be released with
// struo_string_free.
//
// struo_digest_bytes accepts a null data pointer only when len is zero. That
// combination is treated as an empty byte buffer.

char *struo_version(void);

void struo_string_free(char *value);

char *struo_digest_bytes(const uint8_t *data, size_t len);

char *struo_action_key_json(const char *request_json);

char *struo_cache_put_blob_json(const char *request_json);

char *struo_cache_put_file_json(const char *request_json);

char *struo_cache_get_blob_json(const char *request_json);

char *struo_cache_get_blob_to_file_json(const char *request_json);

char *struo_cache_has_blob_json(const char *request_json);

char *struo_cache_put_action_result_json(const char *request_json);

char *struo_cache_get_action_result_json(const char *request_json);

char *struo_cache_forget_action_json(const char *request_json);

char *struo_cache_stats_json(const char *request_json);

#ifdef __cplusplus
}
#endif

#endif
