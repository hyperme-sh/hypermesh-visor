#pragma once
#include <array>
#include <string_view>

namespace visor {
// Accept only canonical arguments from the pinned SDK, never getopt aliases.
inline const char* DatabaseFromArguments(int argc, char* argv[]) {
  if (argc < 2 || argc > 256) return nullptr;
  constexpr std::array<std::string_view, 11> values = {
    "database", "metrics-dir", "http-proxy", "annotation", "attachment",
    "crash-envelope", "report-id", "initial-client-fd", "initial-client-data",
    "client-pid", "handshake-fd"
  };
  std::array<bool, values.size()> seen{};
  const char* database = nullptr;
  std::string_view metrics;
  for (int i = 1; i < argc; ++i) {
    if (!argv[i]) return nullptr;
    const std::string_view argument(argv[i]);
    if (argument == "--no-rate-limit" || argument == "--shared-client-connection") continue;
    if (argument.substr(0, 2) != "--") return nullptr;
    const auto equal = argument.find('=');
    if (equal == std::string_view::npos) return nullptr;
    const auto key = argument.substr(2, equal - 2);
    const auto value = argument.substr(equal + 1);
    bool accepted = false;
    for (size_t j = 0; j < values.size(); ++j) {
      if (key != values[j]) continue;
      if (seen[j] && key != "annotation" && key != "attachment") return nullptr;
      seen[j] = true;
      accepted = true;
      break;
    }
    if (!accepted) return nullptr;
    if (key == "database") {
      if (value.empty()) return nullptr;
      database = argv[i] + equal + 1;
    }
    if (key == "metrics-dir") metrics = value;
  }
  if (!database || (!metrics.empty() && metrics != database)) return nullptr;
  return database;
}
}
