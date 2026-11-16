#pragma once
namespace visor {
// Installed once before HandlerMain creates worker threads. SDK-only callers
// have no permission callback and therefore cannot create raw reports.
inline bool (*capture_permission)(void*) = nullptr;
inline void* capture_owner = nullptr;
inline bool MayWriteRaw() {
  return capture_permission && capture_permission(capture_owner);
}
}
