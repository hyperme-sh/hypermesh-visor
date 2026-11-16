// Unit contract: synthetic bytes only; no native capture or process failure.
#include "client/crash_report_database.h"
#include <cstdio>
#include "util/file/visor_capture_permission.h"
#include <limits>
#include <vector>
int main() {
  using crashpad::CrashReportDatabase;
  auto database = CrashReportDatabase::Initialize(base::FilePath(FILE_PATH_LITERAL("database")));
  if (!database) return 1;
  std::unique_ptr<CrashReportDatabase::NewReport> denied;
  if (database->PrepareNewCrashReport(&denied) == CrashReportDatabase::kNoError) return 16;
  bool allowed = true;
  visor::capture_owner = &allowed;
  visor::capture_permission = [](void* value) { return *static_cast<bool*>(value); };
  const char byte = 'x';
  constexpr size_t limit = 32 * 1024 * 1024;
  for (unsigned i = 0; i < 5; ++i) {
    std::unique_ptr<CrashReportDatabase::NewReport> report;
    if (database->PrepareNewCrashReport(&report) != CrashReportDatabase::kNoError) return 2;
    auto* writer = report->Writer();
    if (report->AddAttachment("private") != nullptr) return 3;
    if (writer->Seek(limit - 1, SEEK_SET) != limit - 1) return 4;
    if (!writer->Write(&byte, 1) || writer->Write(&byte, 1)) return 5;
    if (writer->Seek(0, SEEK_END) != limit) return 6;
    if (writer->Seek(0, SEEK_SET) != 0) return 7;
    if (writer->Write(&byte, std::numeric_limits<size_t>::max())) return 8;
    std::vector<crashpad::WritableIoVec> huge = {{&byte, limit}, {&byte, 1}};
    if (writer->WriteIoVec(&huge)) return 9;
    if (writer->Seek(limit - 1, SEEK_SET) != limit - 1) return 10;
    std::vector<crashpad::WritableIoVec> pair = {{&byte, 1}, {&byte, 1}};
    if (writer->WriteIoVec(&pair)) return 11;
    std::vector<crashpad::WritableIoVec> one = {{&byte, 1}};
    if (!writer->WriteIoVec(&one)) return 12;
    if (writer->Seek(0, SEEK_END) != limit) return 13;
    allowed = false;
    if (writer->Seek(0, SEEK_SET) != 0 || writer->Write(&byte, 1)) return 17;
    std::vector<crashpad::WritableIoVec> revoked = {{&byte, 1}};
    if (writer->WriteIoVec(&revoked)) return 18;
    allowed = true;
    // Failed or discarded reports consume a reservation too.
  }
  std::unique_ptr<CrashReportDatabase::NewReport> sixth;
  if (database->PrepareNewCrashReport(&sixth) == CrashReportDatabase::kNoError) return 14;
  // Reopening a database must not reset the handler's reservations.
  database.reset();
  database = CrashReportDatabase::Initialize(base::FilePath(FILE_PATH_LITERAL("database")));
  if (!database || database->PrepareNewCrashReport(&sixth) == CrashReportDatabase::kNoError) return 15;
  return 0;
}
