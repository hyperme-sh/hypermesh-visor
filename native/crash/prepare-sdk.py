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
text += '\nextern "C" SENTRY_API int sentry_visor_capture_policy_version(void) { return 2; }\n'
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

# Apply write limits in the actual file writer, before any bytes reach disk.
# Only raw report writers opt in; unrelated SDK metadata keeps its semantics.
def checked(path, before, after):
    text = path.read_text()
    if text.count(before) != 1:
        raise SystemExit(f"unexpected capture quota patch anchor: {path.name}")
    path.write_text(text.replace(before, after))

crashpad = source / "external/crashpad"
writer_h = crashpad / "util/file/file_writer.h"
checked(writer_h, "  ScopedFileHandle file_;", """  bool visor_bounded_ = false;
  ScopedFileHandle file_;""")
checked(writer_h, "  void Close();", """  void Close();
  void VisorBoundRawDump() { visor_bounded_ = true; }""")
writer_cc = crashpad / "util/file/file_writer.cc"
checked(writer_cc, "#include <limits.h>", "#include <limits.h>\n#include <cstdio>")
checked(writer_cc, "  return weak_file_handle_file_writer_.Write(data, size);", """  if (visor_bounded_) {
    const auto position = weak_file_handle_file_writer_.Seek(0, SEEK_CUR);
    if (position < 0 || position > 33554432 || size > 33554432 - position)
      return false;
  }
  return weak_file_handle_file_writer_.Write(data, size);""")
checked(writer_cc, "  return weak_file_handle_file_writer_.WriteIoVec(iovecs);", """  if (visor_bounded_) {
    if (iovecs->empty()) return true;
    size_t total = 0;
    for (const auto& item : *iovecs) {
      if (item.iov_len > 33554432 - total) return false;
      total += item.iov_len;
    }
    const auto position = weak_file_handle_file_writer_.Seek(0, SEEK_CUR);
    if (position < 0 || position > 33554432 || total > 33554432 - position)
      return false;
  }
  return weak_file_handle_file_writer_.WriteIoVec(iovecs);""")
database = crashpad / "client/crash_report_database.cc"
checked(database, "#include <sys/stat.h>", "#include <sys/stat.h>\n#include <atomic>")
checked(database, "  database_ = database;", """  // Conservative reservations never reset within this handler process.
  // Owner-wide admission and expiry are separate from this writer bound.
  static std::atomic<unsigned> reservations{0};
  unsigned count = reservations.load();
  do {
    if (count >= 5) return false;
  } while (!reservations.compare_exchange_weak(count, count + 1));
  writer_->VisorBoundRawDump();
  database_ = database;""")
start = database.read_text().index("FileWriter* CrashReportDatabase::NewReport::AddAttachment(")
end = database.read_text().index("\nvoid CrashReportDatabase::UploadReport::InitializeAttachments()", start)
text = database.read_text()
database.write_text(text[:start] + """FileWriter* CrashReportDatabase::NewReport::AddAttachment(
    const std::string& name) {
  // Raw capture has no attachments or auxiliary unbounded output.
  return nullptr;
}
""" + text[end:])

quota = crashpad / "client/visor_quota_contract.cc"
quota.write_text((Path(__file__).parent / "contract/quota.cpp").read_text())
cmake = crashpad / "client/CMakeLists.txt"
cmake.write_text(cmake.read_text() + '''
add_executable(visor-crash-quota-contract visor_quota_contract.cc)
target_link_libraries(visor-crash-quota-contract PRIVATE crashpad_client crashpad_interface)
''')
