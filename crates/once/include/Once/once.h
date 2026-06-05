#ifndef ONCE_H
#define ONCE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

const char *once_version(void);

void once_string_free(char *value);

char *once_digest_bytes(const uint8_t *data, size_t len);

char *once_action_digest_json(const char *action_json);

char *once_run_command_json(const char *request_json);

#ifdef __cplusplus
}
#endif

#endif
