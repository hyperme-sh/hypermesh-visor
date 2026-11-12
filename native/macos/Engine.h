#ifndef HYPERMESH_NATIVE_ENGINE_H
#define HYPERMESH_NATIVE_ENGINE_H
#include <stddef.h>
#include <stdint.h>
// In-process interface only. Response buffers use malloc/free on both sides.
typedef int32_t (*HypermeshNativeCallback)(const uint8_t *, size_t, uint8_t **, size_t *);
int32_t hypermesh_native_start(HypermeshNativeCallback);
int32_t hypermesh_native_control(const uint8_t *, size_t, uint8_t **, size_t *);
#endif
