// Exercise package startup against the app-local desktop Gaming Runtime.
#include <windows.h>
#include <XGameRuntimeInit.h>
#include <XGame.h>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <cstdlib>
#include "scarlett_user_probe.h"

template<typename T>
T symbol(HMODULE module, const char* name) {
    return reinterpret_cast<T>(GetProcAddress(module, name));
}

bool check_http_request(HMODULE lib) {
    auto init = symbol<HRESULT (WINAPI*)(void*)>(lib, "HCInitialize");
    auto cleanup = symbol<void (WINAPI*)()>(lib, "HCCleanup");
    auto create = symbol<HRESULT (WINAPI*)(void**)>(lib, "HCHttpCallCreate");
    auto close = symbol<HRESULT (WINAPI*)(void*)>(lib, "HCHttpCallCloseHandle");
    auto set_url = symbol<HRESULT (WINAPI*)(void*, const char*, const char*)>(lib, "HCHttpCallRequestSetUrl");
    auto get_url = symbol<HRESULT (WINAPI*)(void*, const char**, const char**)>(lib, "HCHttpCallRequestGetUrl");
    if (!init || !cleanup || !create || !close || !set_url || !get_url) return false;
    const HRESULT hr = init(nullptr);
    if (FAILED(hr)) {
        std::fprintf(stderr, "HCInitialize returned %#lx\n", static_cast<unsigned long>(hr));
        return false;
    }
    void* call = nullptr;
    bool ok = SUCCEEDED(create(&call)) && call;
    if (ok) {
        const char* method = nullptr;
        const char* url = nullptr;
        const char* expected = "https://example.invalid/coin-run";
        ok = SUCCEEDED(set_url(call, "POST", expected)) && SUCCEEDED(get_url(call, &method, &url))
            && method && url && !std::strcmp(method, "POST") && !std::strcmp(url, expected);
        ok = SUCCEEDED(close(call)) && ok;
    }
    cleanup();
    return ok;
}

bool is_staged(HMODULE module, const wchar_t* name) {
    wchar_t expected[MAX_PATH], actual[MAX_PATH];
    const DWORD len = GetFullPathNameW(name, MAX_PATH, expected, nullptr);
    const DWORD got = module ? GetModuleFileNameW(module, actual, MAX_PATH) : 0;
    return len && len < MAX_PATH && got && got < MAX_PATH && !_wcsicmp(expected, actual);
}

int main(int argc, char** argv) {
    const bool users = argc == 3 && !std::strcmp(argv[1], "--users");
    const bool with_config = users || (argc == 3 && !std::strcmp(argv[1], "--game-config"));
    if (argc != 1 && !with_config) return 2;
    const XGameRuntimeOptions options{XGameRuntimeGameConfigSource::File, "MicrosoftGame.config"};
    const HRESULT hr = with_config ? XGameRuntimeInitializeWithOptions(&options) : XGameRuntimeInitialize();
    if (FAILED(hr)) {
        std::fprintf(stderr, "Gaming Runtime initialization (%s) returned %#lx\n", with_config ? "package" : "default",
                     static_cast<unsigned long>(hr));
        return 1;
    }
    if (!is_staged(GetModuleHandleW(L"xgameruntime.dll"), L"xgameruntime.dll")) {
        std::fputs("Unexpected Gaming Runtime module path\n", stderr);
        XGameRuntimeUninitialize();
        return 1;
    }
    if (with_config) {
        uint32_t actual = 0;
        const HRESULT id_hr = XGameGetXboxTitleId(&actual);
        const auto expected = std::strtoul(argv[2], nullptr, 16);
        if (FAILED(id_hr) || actual != expected) {
            std::fprintf(stderr, "Title ID expected %#lx, got %#x (HRESULT %#lx)\n",
                         expected, actual, static_cast<unsigned long>(id_hr));
            XGameRuntimeUninitialize();
            return 1;
        }
        std::printf("Gaming Runtime reports package title ID %#x.\n", actual);
    }
    if (users) {
        const bool ok = check_package_users();
        XGameRuntimeUninitialize();
        return !ok;
    }
    HMODULE lib = LoadLibraryW(L".\\libHttpClient.GDK.dll");
    bool ok = false;
    if (lib) {
        if (is_staged(lib, L"libHttpClient.GDK.dll")) {
            ok = check_http_request(lib);
        } else {
            std::fputs("Unexpected Gaming Runtime or HTTP client module path\n", stderr);
        }
        FreeLibrary(lib);
    } else {
        std::fprintf(stderr, "Scarlett HTTP client load failed (%lu)\n", GetLastError());
    }
    XGameRuntimeUninitialize();
    std::puts(ok ? "Scarlett HTTP request setup passed." : "Scarlett HTTP request setup failed.");
    return !ok;
}
