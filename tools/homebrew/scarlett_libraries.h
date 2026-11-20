// Offline checks for the supplied x64 Scarlett runtimes. The launcher puts the
// audited DLLs and Coin Run together in an isolated application directory.
#pragma once
#include <atomic>
#include <cmath>
#include <cwchar>
#include <thread>

namespace scarlett_libraries {

template<typename T>
T symbol(HMODULE module, const char* name) {
    return reinterpret_cast<T>(GetProcAddress(module, name));
}

inline bool check_memory_query() {
    using Query = BOOL (WINAPI*)(HANDLE, const void*, void*);
    auto query = symbol<Query>(GetModuleHandleW(L"xmem.dll"), "XMemVirtualQuery");
    if (!query) return false;
    SYSTEM_INFO system{};
    GetSystemInfo(&system);
    BYTE* base = static_cast<BYTE*>(VirtualAlloc(nullptr, 2 * system.dwPageSize, MEM_RESERVE, PAGE_NOACCESS));
    if (!base) return false;
    struct Result {
        UINT64 before;
        MEMORY_BASIC_INFORMATION memory;
        UINT64 console[2];
        UINT64 after;
    };
    static_assert(sizeof(MEMORY_BASIC_INFORMATION) == 48);
    Result out{};
    auto matches = [&](const void* address, DWORD state, DWORD protect) {
        std::memset(&out, 0xa5, sizeof(out));
        return query(GetCurrentProcess(), address, &out.memory) &&
            out.memory.AllocationBase == base && out.memory.State == state &&
            (state != MEM_COMMIT || out.memory.Protect == protect) &&
            !out.console[0] && !out.console[1] &&
            out.before == 0xa5a5a5a5a5a5a5a5ull && out.after == out.before;
    };
    bool ok = matches(base + system.dwPageSize, MEM_RESERVE, 0) &&
        VirtualAlloc(base, system.dwPageSize, MEM_COMMIT, PAGE_READONLY) == base &&
        matches(base, MEM_COMMIT, PAGE_READONLY) &&
        matches(base + system.dwPageSize, MEM_RESERVE, 0);
    const auto saved = out;
    ok = ok && !query(nullptr, base, &out.memory) && !std::memcmp(&out, &saved, sizeof(out))
        && !query(GetCurrentProcess(), base, nullptr);
    VirtualFree(base, 0, MEM_RELEASE);
    return ok;
}

inline bool check_game_chat() {
    HMODULE chat = LoadLibraryW(L"GameChat2.dll");
    if (!chat) { std::fprintf(stderr, "GameChat2 load failed (%lu)\n", GetLastError()); return false; }
    auto get = symbol<HRESULT (WINAPI*)(int, UINT64*)>(chat, "ChatManagerGetThreadAffinityMask");
    auto set = symbol<HRESULT (WINAPI*)(int, UINT64)>(chat, "ChatManagerSetThreadAffinityMask");
    DWORD_PTR process_mask = 0, system_mask = 0;
    bool ok = get && set && GetProcessAffinityMask(GetCurrentProcess(), &process_mask, &system_mask) && process_mask;
    // The public C ABI assigns thread IDs 0 and 1 to audio and networking.
    for (int thread = 0; ok && thread < 2; ++thread) {
        UINT64 initial = 0, actual = 0;
        if (FAILED(get(thread, &initial))) { ok = false; break; }
        const UINT64 selected = process_mask & (0 - process_mask);
        ok = SUCCEEDED(set(thread, selected)) && SUCCEEDED(get(thread, &actual)) && actual == selected;
        ok = SUCCEEDED(set(thread, initial)) && SUCCEEDED(get(thread, &actual)) && actual == initial && ok;
    }
    FreeLibrary(chat);
    std::puts(ok ? "GameChat2 audio and networking affinity round trips passed." : "GameChat2 affinity check failed.");
    return ok;
}

inline bool check_atomic_wait(HMODULE lib) {
    auto wait = symbol<int (WINAPI*)(const void*, void*, size_t, DWORD)>(lib, "__std_atomic_wait_direct");
    auto wake = symbol<void (WINAPI*)(const void*)>(lib, "__std_atomic_notify_one_direct");
    LONG word = 7, equal = word, different = 11;
    if (!wait || !wake || wait(&word, &equal, sizeof(word), 0) || GetLastError() != ERROR_TIMEOUT ||
        !wait(&word, &different, sizeof(word), 0)) return false;

    // Keep the compared word equal. Retry the notification if the sender runs before the wait.
    std::atomic<bool> done{false};
    std::thread sender([&] {
        do {
            wake(&word);
            Sleep(1);
        } while (!done.load());
    });
    const bool woke = wait(&word, &equal, sizeof(word), 5000) != 0;
    done.store(true);
    sender.join();
    return woke;
}

inline bool check(HMODULE lib, const char* name) {
    if (!std::strcmp(name, "XboxBvhBuilder.dll")) {
        return GetProcAddress(lib, "?ValidateRehydrateAccelerationStructureBuildDesc@XboxBvhBuilder@@QEAAJPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@@Z")
            && check_memory_query();
    }
    if (!std::strcmp(name, "Bink2Scarlett.dll")) {
        auto set = symbol<void (*)(const char*)>(lib, "BinkSetError");
        auto get = symbol<const char* (*)()>(lib, "BinkGetError");
        if (!set || !get) return false;
        set("Coin Run Bink test");
        const char* error = get();
        return error && !std::strcmp(error, "Coin Run Bink test");
    }
    if (!std::strcmp(name, "XCurl.dll")) {
        auto unescape = symbol<char* (*)(void*, const char*, int, int*)>(lib, "curl_easy_unescape");
        auto release = symbol<void (*)(void*)>(lib, "curl_free");
        if (!unescape || !release) return false;
        int size = 0;
        char* data = unescape(nullptr, "A%00B", 5, &size);
        const bool pass = data && size == 3 && !std::memcmp(data, "A\0B", 3);
        release(data);
        return pass;
    }
    if (!std::strcmp(name, "libHttpClient.GDK.dll")) {
        auto version = symbol<HRESULT (*)(const char**)>(lib, "HCGetLibVersion");
        const char* value = nullptr;
        return version && SUCCEEDED(version(&value)) && value && *value;
    }
    if (!std::strcmp(name, "vcruntime140.dll")) {
        auto move = symbol<void* (*)(void*, const void*, size_t)>(lib, "memmove");
        char overlap[] = "0123456";
        return move && move(overlap+1, overlap, 5) == overlap+1 && !std::strcmp(overlap, "0012346");
    }
    if (!std::strcmp(name, "msvcp140.dll")) {
        auto count = symbol<int (*)()>(lib, "?uncaught_exceptions@std@@YAHXZ");
        return count && count() == 0;
    }
    if (!std::strcmp(name, "msvcp140_1.dll")) {
        auto get = symbol<void* (*)()>(lib, "_Aligned_get_default_resource");
        auto pool = symbol<void* (*)()>(lib, "_Aligned_new_delete_resource");
        return get && pool && get() && get() == pool();
    }
    if (!std::strcmp(name, "msvcp140_2.dll")) {
        auto beta = symbol<double (*)(double,double)>(lib, "__std_smf_beta");
        return beta && std::abs(beta(2.0,3.0) - 1.0/12.0) < 1e-12;
    }
    if (!std::strcmp(name, "msvcp140_atomic_wait.dll")) {
        auto allocate = symbol<void* (*)(size_t,size_t)>(lib, "__std_calloc_crt");
        auto release = symbol<void (*)(void*)>(lib, "__std_free_crt");
        if (!allocate || !release) return false;
        auto words = static_cast<unsigned*>(allocate(4, sizeof(unsigned)));
        const bool pass = words && !(words[0] | words[1] | words[2] | words[3]);
        release(words);
        return pass && check_atomic_wait(lib);
    }
    if (!std::strcmp(name, "vcomp140.dll")) {
        auto num_threads = symbol<int (*)()>(lib, "omp_get_num_threads");
        auto thread_num = symbol<int (*)()>(lib, "omp_get_thread_num");
        return num_threads && thread_num && num_threads() == 1 && thread_num() == 0;
    }
    if (!std::strcmp(name, "msvcp140_codecvt_ids.dll")) {
        auto narrow = GetProcAddress(lib, "?id@?$codecvt@_SDU_Mbstatet@@@std@@2V0locale@2@A");
        auto wide = GetProcAddress(lib, "?id@?$codecvt@_UDU_Mbstatet@@@std@@2V0locale@2@A");
        return narrow && wide && narrow != wide;
    }
    if (!std::strcmp(name, "concrt140.dll")) {
        auto get = symbol<unsigned (*)()>(lib, "?GetProcessorCount@Concurrency@@YAIXZ");
        const unsigned count = get ? get() : 0;
        return count > 0 && count <= GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
    }
    if (!std::strcmp(name, "vccorlib140.dll"))
        return GetProcAddress(lib, "?InitializeData@Details@Platform@@YAJH@Z") != nullptr;
    if (!std::strcmp(name, "vcruntime140_1.dll"))
        return GetProcAddress(lib, "__CxxFrameHandler4") != nullptr;
    return false;
}

inline int run() {
    struct Probe { const char* dll; const char* operation; };
    const Probe probes[] = {
        {"vcruntime140.dll","overlapping memmove"}, {"vcruntime140_1.dll","exception handler export"},
        {"msvcp140.dll","uncaught exception count"}, {"msvcp140_1.dll","default memory resource"},
        {"msvcp140_2.dll","beta(2,3)"}, {"msvcp140_atomic_wait.dll","zeroed allocation, wait timeout, unequal value and cross-thread wake"},
        {"msvcp140_codecvt_ids.dll","distinct exported facet IDs"}, {"concrt140.dll","available processor count"},
        {"vcomp140.dll","serial OpenMP team"}, {"vccorlib140.dll","initialization export"},
        {"XboxBvhBuilder.dll","native load and XMemVirtualQuery memory-region checks"},
        {"Bink2Scarlett.dll","error string round trip"}, {"libHttpClient.GDK.dll","library version"},
        {"XCurl.dll","URL decode with embedded NUL"},
    };
    HMODULE api = LoadLibraryW(L"api-ms-win-core-libraryloader-l1-2-0.dll");
    auto from_address = api ? symbol<BOOL (WINAPI*)(DWORD, LPCWSTR, HMODULE*)>(api, "GetModuleHandleExW") : nullptr;
    auto filename = symbol<DWORD (WINAPI*)(HMODULE, LPWSTR, DWORD)>(GetModuleHandleW(L"kernel32.dll"), "GetModuleFileNameW");
    if (!filename || !from_address) { if (api) FreeLibrary(api); return 1; }
    int failures = 0;
    for (const auto& p : probes) {
        wchar_t name[MAX_PATH], expected[MAX_PATH], actual[MAX_PATH];
        if (!MultiByteToWideChar(CP_UTF8,0,p.dll,-1,name,MAX_PATH)) return 1;
        const DWORD len = GetFullPathNameW(name,MAX_PATH,expected,nullptr);
        if (!len || len >= MAX_PATH) return 1;
        const HMODULE module = LoadLibraryW(expected);
        if (!module) {
            std::fprintf(stderr, "%s load failed (%lu)\n", p.dll, GetLastError());
            ++failures;
            continue;
        }
        HMODULE owner = nullptr;
        const bool owns = from_address(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                       reinterpret_cast<LPCWSTR>(module), &owner) && owner == module;
        const DWORD count = filename(module,actual,MAX_PATH);
        const bool local = owns && count && count < MAX_PATH && !_wcsicmp(actual,expected);
        const bool passed = local && check(module,p.dll);
        std::printf("%s %s: %s\n", p.dll, p.operation, passed ? "passed" : (local ? "failed" : "unexpected module path"));
        failures += !passed;
        FreeLibrary(module);
    }
    FreeLibrary(api);
    return failures != 0;
}
}
