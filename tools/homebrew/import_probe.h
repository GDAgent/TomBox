#pragma once
#include <windows.h>
#include <cstdio>
#include <cstdlib>
#include <map>
#include <string>

namespace {
const char *scenario;
bool verify_native_ucrt;
using Import = std::pair<std::string, std::string>;
std::map<Import, std::string> checked;

void fail(const char *reason, int line) {
    std::fprintf(stderr, "%s (line %d): %s\n", scenario, line, reason);
    std::exit(1);
}
#define REQUIRE(expr)                                                                                              \
    do {                                                                                                           \
        if (!(expr))                                                                                               \
            fail(#expr, __LINE__);                                                                                 \
    } while (false)

// The launcher places the supplied native DLLs beside the probe and runs it there.
void require_app_local_module(HMODULE module, const char *basename) {
    char loaded[MAX_PATH], expected[MAX_PATH];
    auto n = GetModuleFileNameA(module, loaded, MAX_PATH);
    auto m = GetFullPathNameA(basename, MAX_PATH, expected, nullptr);
    REQUIRE(n && n < MAX_PATH && m && m < MAX_PATH && !_stricmp(loaded, expected));
}

struct Module {
    const char *name;
    HMODULE handle;
    explicit Module(const char *dll, DWORD search = LOAD_LIBRARY_SEARCH_SYSTEM32)
        : name(dll), handle(LoadLibraryExA(dll, nullptr, search)) {
        REQUIRE(handle);
        if (search == LOAD_LIBRARY_SEARCH_APPLICATION_DIR)
            require_app_local_module(handle, dll);
    }
    ~Module() {
        FreeLibrary(handle);
    }
    Module(const Module &) = delete;
    Module &operator=(const Module &) = delete;

    template <typename Fn> Fn function(const char *symbol, WORD ordinal = 0) const {
        auto proc = GetProcAddress(handle, ordinal ? MAKEINTRESOURCEA(ordinal) : symbol);
        if (!proc)
            fail((std::string(name) + "!" + symbol).c_str(), __LINE__);
        if (verify_native_ucrt) {
            HMODULE owner = nullptr;
            REQUIRE(GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                           GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                       reinterpret_cast<const char *>(proc), &owner));
            require_app_local_module(owner, "ucrtbase.dll");
        }
        checked.emplace(Import{name, symbol}, scenario);
        return reinterpret_cast<Fn>(proc);
    }
};
#define API(module, fn) (module.function<decltype(&::fn)>(#fn))

} // namespace
