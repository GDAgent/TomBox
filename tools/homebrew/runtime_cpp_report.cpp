// Kept outside the instrumented fixture so reporting does not earn coverage.
#include "import_probe.h"

extern "C" void runtime_scenario(const char *label) noexcept {
    scenario = label;
    for (auto name : {"msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"}) {
        auto module = GetModuleHandleA(name);
        REQUIRE(module);
        require_app_local_module(module, name);
    }
}

extern "C" void runtime_hit(const char *module, const char *symbol, const void *target) noexcept {
    if (!scenario)
        return;
    HMODULE dll = GetModuleHandleA(module);
    REQUIRE(dll && reinterpret_cast<const void *>(GetProcAddress(dll, symbol)) == target);
    checked.emplace(Import{module, symbol}, scenario);
}

extern "C" int runtime_report() noexcept {
    REQUIRE(!checked.empty());
    for (const auto &[key, evidence] : checked)
        std::printf("%s\t%s\t%s\n", key.first.c_str(), key.second.c_str(), evidence.c_str());
    scenario = nullptr;
    return std::fflush(stdout) != 0 || std::ferror(stdout);
}
