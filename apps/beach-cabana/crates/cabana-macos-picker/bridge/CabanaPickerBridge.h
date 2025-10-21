#pragma once

#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*CabanaPickerEventCallback)(const char *json_utf8, void *user_data);

bool cabana_picker_is_available(void);

/**
 * Creates a new picker controller. Returns NULL on failure and sets `error_message`
 * to a newly allocated UTF-8 string that must be freed with `cabana_picker_free_c_string`.
 */
void *cabana_picker_create(CabanaPickerEventCallback callback,
                           void *user_data,
                           const char **error_message);

/**
 * Presents the native picker. Returns false on failure and sets `error_message`.
 */
bool cabana_picker_present(void *handle, const char **error_message);

/**
 * Cancels any active picker presentation and removes observers.
 */
void cabana_picker_cancel(void *handle);

/**
 * Destroys the picker controller, removing observers and releasing resources.
 */
void cabana_picker_destroy(void *handle);

/**
 * Frees a UTF-8 string allocated by the bridge.
 */
void cabana_picker_free_c_string(const char *ptr);

#ifdef __cplusplus
}
#endif
