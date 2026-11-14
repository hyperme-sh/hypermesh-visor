#include "../handler-arguments.h"
#include <initializer_list>
#include <string>
#include <vector>
static bool accepted(std::initializer_list<const char*> values) {
  std::vector<std::string> strings;
  for (const auto* value : values) strings.emplace_back(value);
  std::vector<char*> args;
  for (auto& value : strings) args.push_back(value.data());
  return visor::DatabaseFromArguments(static_cast<int>(args.size()), args.data()) != nullptr;
}
int main() {
  if (!accepted({"handler", "--database=/private/run/database", "--metrics-dir=/private/run/database",
      "--no-rate-limit", "--annotation=release=fixture", "--annotation=another=fixture",
      "--initial-client-fd=4", "--shared-client-connection"})) return 1;
  if (!accepted({"handler", "--database=C:\\private\\database", "--initial-client-data=1,2,3"})) return 2;
  if (!accepted({"handler", "--database=/private/database", "--client-pid=42", "--handshake-fd=5"})) return 3;
  for (const char* option : {"--url=https://invalid", "--ur=https://invalid", "--u=https://invalid",
      "--dat=/other", "--database=/other", "--database", "--monitor-self",
      "--log-file=/other", "--crash-reporter=/other", "--metrics-dir=/other", "--", "-u"}) {
    if (accepted({"handler", "--database=/private/database", option})) return 4;
  }
  if (accepted({"handler", "--database=/private/database", "--database", "/other"})) return 5;
  return 0;
}
