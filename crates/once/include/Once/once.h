#ifndef ONCE_H
#define ONCE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// All functions are thread-safe. Returned char pointers are owned by Once
// and must be released with once_string_free. Cache functions use a shared
// runtime with 2 worker threads by default. Set ONCE_FFI_WORKER_THREADS to
// override that count.

char *once_version(void);

void once_string_free(char *value);

char *once_digest_bytes(const uint8_t *data, size_t len);

char *once_action_digest_json(const char *action_json);

char *once_cache_put_blob_json(const char *request_json);

char *once_cache_get_blob_json(const char *request_json);

char *once_cache_has_blob_json(const char *request_json);

char *once_cache_put_action_result_json(const char *request_json);

char *once_cache_get_action_result_json(const char *request_json);

char *once_cache_forget_action_json(const char *request_json);

char *once_cache_stats_json(const char *request_json);

#ifdef __cplusplus
}
#endif

#endif
