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
old = '        arguments.push_back(std::string("--log-file=") + log_path->path);'
if text.count(old) != 1:
    raise SystemExit("unexpected native handler log configuration")
text = text.replace(old, "        // Visor captures no separate handler log.")
text += '\nextern "C" SENTRY_API int sentry_visor_capture_policy_version(void) { return 1; }\n'
backend.write_text(text)

main = source / "external/crashpad/handler/main.cc"
text = main.read_text()
old = "return crashpad::HandlerMain(argc, argv, nullptr);"
if text.count(old) != 2:
    raise SystemExit("unexpected native handler entry points")
text = text.replace(old, "return VisorHandlerMain(argc, argv);")
hook = r'''
#include "visor_handler_arguments.h"
extern "C" void* visor_capture_enter(const char* database);
extern "C" void visor_capture_leave(void* guard);
static int VisorHandlerMain(int argc, char* argv[]) {
  const char* database = visor::DatabaseFromArguments(argc, argv);
  if (!database) return 1;
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
(main.parent / "visor_handler_arguments.h").write_text((Path(__file__).parent / "handler-arguments.h").read_text())

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
