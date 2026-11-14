"""Apply narrow, checked capture-ownership hooks to the pinned SDK source."""
from pathlib import Path
import sys

source = Path(sys.argv[1])
backend = source / "src/backends/sentry_backend_crashpad.cpp"
text = backend.read_text()
old = "/* restartable */ true,"
if text.count(old) != 1:
    raise SystemExit("unexpected native handler restart configuration")
text = text.replace(old, "/* restartable */ false,")
old = "    crashpad_register_wer_module(absolute_handler_path, data);"
if text.count(old) != 1:
    raise SystemExit("unexpected native WER configuration")
text = text.replace(old, "    // Visor uses only its consent-owned handler, never WER fallback.")
text += '\nextern "C" SENTRY_API int sentry_visor_capture_policy_version(void) { return 1; }\n'
backend.write_text(text)

main = source / "external/crashpad/handler/main.cc"
text = main.read_text()
old = "return crashpad::HandlerMain(argc, argv, nullptr);"
if text.count(old) != 2:
    raise SystemExit("unexpected native handler entry points")
text = text.replace(old, "return VisorHandlerMain(argc, argv);")
hook = r'''
#include <cstring>
extern "C" void* visor_capture_enter(const char* database);
extern "C" void visor_capture_leave(void* guard);
static int VisorHandlerMain(int argc, char* argv[]) {
  const char* database = nullptr;
  for (int i = 1; i < argc; ++i) {
    // A guarded handler never accepts an upload destination, even if a caller
    // accidentally configures the native SDK with an ingest credential.
    if (std::strncmp(argv[i], "--url", 5) == 0) return 1;
    if (std::strncmp(argv[i], "--database=", 11) == 0) {
      if (database) return 1;
      database = argv[i] + 11;
    }
  }
  void* guard = visor_capture_enter(database);
  if (!guard) return 1;
  const int result = crashpad::HandlerMain(argc, argv, nullptr);
  visor_capture_leave(guard);
  return result;
}
'''
anchor = "#if BUILDFLAG(IS_POSIX)"
if text.count(anchor) != 1:
    raise SystemExit("unexpected native handler platform branches")
main.write_text(text.replace(anchor, hook + "\n" + anchor))

cmake = source / "external/crashpad/handler/CMakeLists.txt"
cmake.write_text(cmake.read_text() + '''
if(NOT DEFINED VISOR_CAPTURE_GUARD)
  message(FATAL_ERROR "Visor capture guard library is required")
endif()
target_link_libraries(crashpad_handler PRIVATE "${VISOR_CAPTURE_GUARD}")
if(WIN32)
  target_link_libraries(crashpad_handler PRIVATE userenv ws2_32 ntdll bcrypt)
elseif(APPLE)
  target_link_libraries(crashpad_handler PRIVATE "-framework Security")
else()
  target_link_libraries(crashpad_handler PRIVATE pthread dl m)
endif()
''')
