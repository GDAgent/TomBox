// Homebrew behavior checks for the exact host DLL names and ordinals in the Scarlett import audit.
// Run in a private working directory. A failed assertion exits before any coverage is published.
#define _CRT_SECURE_NO_WARNINGS // The audited binaries import these CRT entry points.
#define NOMINMAX
#define _WIN32_WINNT 0x0a00
#include <windows.h>
#include <oleauto.h>
#include <winstring.h>
#include <winhttp.h>
#include <roapi.h>
#include <wincrypt.h>
#include <locale.h>
#include <corecrt_startup.h>
#include <process.h>
#include <vector>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <io.h>
#include <fcntl.h>
#include <malloc.h>
#include <map>
#include <string>
#include <sys/stat.h>
#include <share.h>
#include <thread>

namespace {
const char* scenario;
using Import = std::pair<std::string, std::string>;
std::map<Import, std::string> checked;

void fail(const char* reason, int line) {
    std::fprintf(stderr, "%s (line %d): %s\n", scenario, line, reason);
    std::exit(1);
}
#define REQUIRE(expr) do { if (!(expr)) fail(#expr, __LINE__); } while (false)

struct Module {
    const char* name;
    HMODULE handle;
    explicit Module(const char* dll) : name(dll), handle(LoadLibraryExA(dll, nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32)) {
        REQUIRE(handle);
    }
    ~Module() { FreeLibrary(handle); }
    Module(const Module&) = delete;
    Module& operator=(const Module&) = delete;

    template<typename Fn>
    Fn function(const char* symbol, WORD ordinal = 0) const {
        auto proc = GetProcAddress(handle, ordinal ? MAKEINTRESOURCEA(ordinal) : symbol);
        if (!proc) fail((std::string(name) + "!" + symbol).c_str(), __LINE__);
        checked.emplace(Import{name, symbol}, scenario);
        return reinterpret_cast<Fn>(proc);
    }
};
#define API(module, fn) (module.function<decltype(&::fn)>(#fn))

void automation_strings() {
    Module ole("oleaut32.dll");
    const wchar_t text[] = L"A\0\u03a9";
    BSTR value = ole.function<decltype(&SysAllocStringLen)>("#4", 4)(text, 3);
    REQUIRE(value && value != text && !std::memcmp(value, text, sizeof(text)));
    UINT bytes;
    std::memcpy(&bytes, reinterpret_cast<const BYTE*>(value) - sizeof(bytes), sizeof(bytes));
    REQUIRE(bytes == 3 * sizeof(wchar_t));
    ole.function<decltype(&SysFreeString)>("#6", 6)(value);
}

void winrt_strings() {
    Module str("api-ms-win-core-winrt-string-l1-1-0.dll");
    const wchar_t text[] = L"A\0\u03a9";
    HSTRING_HEADER header{};
    HSTRING ref{}, owned{}, copy{};
    REQUIRE(SUCCEEDED(API(str, WindowsCreateStringReference)(text, 3, &header, &ref)));
    REQUIRE(SUCCEEDED(API(str, WindowsCreateString)(text, 3, &owned)));
    REQUIRE(SUCCEEDED(API(str, WindowsDuplicateString)(owned, &copy)));
    REQUIRE(SUCCEEDED(API(str, WindowsDeleteString)(owned)));
    BOOL embedded = FALSE;
    REQUIRE(SUCCEEDED(API(str, WindowsStringHasEmbeddedNull)(copy, &embedded)) && embedded);
    REQUIRE(API(str, WindowsGetStringLen)(copy) == 3);
    REQUIRE(API(str, WindowsIsStringEmpty)(nullptr) && !API(str, WindowsIsStringEmpty)(ref));
    INT32 order = 1;
    REQUIRE(SUCCEEDED(API(str, WindowsCompareStringOrdinal)(ref, copy, &order)) && order == 0);
    UINT32 count = 0;
    const auto buffer = API(str, WindowsGetStringRawBuffer)(copy, &count);
    REQUIRE(buffer && count == 3 && !std::memcmp(buffer, text, sizeof(text)));
    REQUIRE(SUCCEEDED(API(str, WindowsDeleteString)(copy)));
    REQUIRE(SUCCEEDED(API(str, WindowsDeleteString)(ref)));
}

void unicode_text() {
    Module kernel("kernel32.dll"), lang("api-ms-win-core-localization-l1-2-0.dll");
    Module str("api-ms-win-core-string-l1-1-0.dll"), old("api-ms-win-core-string-obsolete-l1-1-0.dll");
    const char utf8[] = "A\xce\xa9";
    wchar_t wide[8]{};
    REQUIRE(API(kernel, MultiByteToWideChar)(CP_UTF8, MB_ERR_INVALID_CHARS, utf8, -1, wide, 8) == 3);
    REQUIRE(wide[0] == L'A' && wide[1] == L'\u03a9' && !wide[2]);
    char bytes[8]{};
    REQUIRE(API(kernel, WideCharToMultiByte)(CP_UTF8, 0, wide, -1, bytes, 8, nullptr, nullptr) == 4);
    REQUIRE(!std::memcmp(utf8, bytes, sizeof(utf8)));
    CPINFO cp{};
    REQUIRE(API(lang, GetCPInfo)(CP_UTF8, &cp) && cp.MaxCharSize == 4);
    REQUIRE(API(old, lstrlenW)(wide) == 2 && API(old, lstrcmpiW)(L"CoIn", L"coin") == 0);
    REQUIRE(API(str, CompareStringEx)(LOCALE_NAME_INVARIANT, 0, L"a", 1, L"b", 1, nullptr, nullptr, 0) == CSTR_LESS_THAN);
    REQUIRE(API(lang, LCMapStringEx)(LOCALE_NAME_INVARIANT, LCMAP_UPPERCASE, L"coin", -1, wide, 8, nullptr, nullptr, 0) == 5);
    REQUIRE(!std::wcscmp(wide, L"COIN"));
    WORD types[2]{};
    REQUIRE(API(str, GetStringTypeW)(CT_CTYPE1, L"A7", 2, types));
    REQUIRE((types[0] & C1_UPPER) && (types[1] & C1_DIGIT));
    REQUIRE(API(str, GetStringTypeExW)(LOCALE_INVARIANT, CT_CTYPE1, L"9a", 2, types));
    REQUIRE((types[0] & C1_DIGIT) && (types[1] & C1_LOWER));
}

void numbers() {
    Module conv("api-ms-win-crt-convert-l1-1-0.dll"), math("api-ms-win-crt-math-l1-1-0.dll");
    wchar_t wide[16]{};
    using ItoW = errno_t (*)(int, wchar_t*, size_t, int);
    REQUIRE(conv.function<ItoW>("_itow_s")(-314, wide, 16, 10) == 0 && !std::wcscmp(wide, L"-314"));
    REQUIRE(API(conv, atol)(" -314x") == -314);
    REQUIRE(API(conv, btowc)('Q') == L'Q' && API(conv, btowc)(EOF) == WEOF);
    wchar_t* wend = nullptr;
    REQUIRE(API(conv, wcstoull)(L"1fffffffe!", &wend, 16) == 0x1fffffffeULL && *wend == L'!');
    char* end = nullptr;
    REQUIRE(API(conv, strtof)("-1.25e2!", &end) == -125.0f && *end == '!');
    char narrow[16]{};
    REQUIRE(API(conv, wcstombs)(narrow, wide, sizeof(narrow)) == 4 && !std::strcmp(narrow, "-314"));
    size_t written = 0;
    using Convert = errno_t (*)(size_t*, char*, size_t, const wchar_t*, size_t);
    REQUIRE(conv.function<Convert>("wcstombs_s")(&written, narrow, sizeof(narrow), L"hello", 5) == 0);
    REQUIRE(written == 6 && !std::strcmp(narrow, "hello"));
    REQUIRE(API(math, ceilf)(-2.75f) == -2 && API(math, ceilf)(2.25f) == 3);
    using Unary = double (*)(double);
    REQUIRE(std::abs(math.function<Unary>("expm1")(1.) - 1.718281828459045) < 1e-12);
    REQUIRE(std::abs(math.function<Unary>("log1p")(1.) - 0.6931471805599453) < 1e-12);
    REQUIRE(math.function<Unary>("log2")(1024.) == 10.);
    REQUIRE(std::abs(math.function<Unary>("sinh")(-1.) + 1.1752011936438014) < 1e-12);
    REQUIRE(std::abs(math.function<Unary>("cosh")(-1.) - 1.5430806348152437) < 1e-12);
    auto float_sign = API(math, _fdsign);
    auto long_sign = API(math, _ldsign);
    REQUIRE(float_sign(-0.0f) && !float_sign(0.0f));
    REQUIRE(long_sign(-0.0L) && !long_sign(0.0L));
}

void runtime_callbacks() {
    Module crt("api-ms-win-crt-runtime-l1-1-0.dll");
    REQUIRE(API(crt, _configure_wide_argv)(_crt_argv_unexpanded_arguments) == 0);
    int* argc = API(crt, __p___argc)();
    wchar_t*** argv = API(crt, __p___wargv)();
    REQUIRE(argc && *argc == 3 && argv && *argv && !(*argv)[3]);
    REQUIRE(!std::wcscmp((*argv)[1], L"coin-import") && !std::wcscmp((*argv)[2], L"17"));

    static int order;
    _onexit_table_t table{};
    REQUIRE(API(crt, _initialize_onexit_table)(&table) == 0);
    auto add = API(crt, _register_onexit_function);
    REQUIRE(add(&table, [] { order = order * 10 + 1; return 0; }) == 0);
    REQUIRE(add(&table, [] { order = order * 10 + 2; return 0; }) == 0);
    REQUIRE(API(crt, _execute_onexit_table)(&table) == 0 && order == 21);
}

void thread_lifecycle() {
    Module crt("api-ms-win-crt-runtime-l1-1-0.dll"), proc("api-ms-win-core-processthreads-l1-1-0.dll");
    Module pool("api-ms-win-core-threadpool-legacy-l1-1-0.dll");
    struct Task {
        decltype(&_endthreadex) finish;
        DWORD id;
    } task{API(crt, _endthreadex), 0};
    auto worker = [](void* raw) -> unsigned {
        auto* task = static_cast<Task*>(raw);
        task->id = GetCurrentThreadId();
        task->finish(42);
        return 1;
    };
    unsigned id = 0;
    auto thread = reinterpret_cast<HANDLE>(API(crt, _beginthreadex)(nullptr, 0, worker, &task, CREATE_SUSPENDED, &id));
    DWORD code = 0;
    auto exit_code = API(proc, GetExitCodeThread);
    REQUIRE(thread && id && exit_code(thread, &code) && code == STILL_ACTIVE);
    REQUIRE(API(proc, SetThreadPriority)(thread, THREAD_PRIORITY_BELOW_NORMAL));
    REQUIRE(API(proc, GetThreadPriority)(thread) == THREAD_PRIORITY_BELOW_NORMAL);
    REQUIRE(API(proc, ResumeThread)(thread) == 1 && WaitForSingleObject(thread, 5000) == WAIT_OBJECT_0);
    REQUIRE(task.id == id && exit_code(thread, &code) && code == 42);
    REQUIRE(CloseHandle(thread));

    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    REQUIRE(event && API(pool, QueueUserWorkItem)([](void* event) -> DWORD {
        return SetEvent(event) ? 0 : 1;
    }, event, WT_EXECUTEDEFAULT));
    REQUIRE(WaitForSingleObject(event, 5000) == WAIT_OBJECT_0 && CloseHandle(event));
}

void crt_strings() {
    Module str("api-ms-win-crt-string-l1-1-0.dll");
    using Predicate = int (*)(int);
    REQUIRE(str.function<Predicate>("isalnum")('8') && !str.function<Predicate>("isalnum")('!'));
    REQUIRE(str.function<Predicate>("islower")('z') && !str.function<Predicate>("islower")('Z'));
    REQUIRE(str.function<Predicate>("isupper")('Z') && !str.function<Predicate>("isupper")('z'));
    REQUIRE(str.function<Predicate>("isxdigit")('F') && !str.function<Predicate>("isxdigit")('G'));
    const char raw[] = {'a', 'b', 'c'};
    using Count = size_t (*)(const char*, size_t);
    REQUIRE(str.function<Count>("__strncnt")(raw, 3) == 3 && str.function<Count>("__strncnt")("a", 3) == 1);
    char buf[8]{};
    using Copy = errno_t (*)(char*, size_t, const char*, size_t);
    REQUIRE(str.function<Copy>("strncpy_s")(buf, sizeof(buf), raw, 2) == 0);
    REQUIRE(API(str, strlen)(buf) == 2 && !std::strcmp(buf, "ab"));
    wchar_t wbuf[12]{};
    using WCopy = errno_t (*)(wchar_t*, size_t, const wchar_t*, size_t);
    using WCat = errno_t (*)(wchar_t*, size_t, const wchar_t*);
    REQUIRE(str.function<WCopy>("wcsncpy_s")(wbuf, 12, L"coin", 2) == 0);
    REQUIRE(str.function<WCat>("wcscat_s")(wbuf, 12, L"in") == 0);
    REQUIRE(str.function<WCopy>("wcsncat_s")(wbuf, 12, L"run?", 3) == 0);
    REQUIRE(API(str, wcslen)(wbuf) == 7 && API(str, wcsnlen)(wbuf, 3) == 3 && !std::wcscmp(wbuf, L"coinrun"));
}

void allocations() {
    Module kernel("kernel32.dll"), local("api-ms-win-core-heap-l2-1-0.dll");
    Module heap("api-ms-win-core-heap-l1-1-0.dll"), crt("api-ms-win-crt-heap-l1-1-0.dll");
    auto block = static_cast<BYTE*>(API(local, LocalAlloc)(LPTR, 24));
    REQUIRE(block && block[0] == 0 && block[23] == 0);
    block[23] = 17;
    REQUIRE(API(local, LocalFree)(block) == nullptr);
    HANDLE process_heap = GetProcessHeap();
    auto data = static_cast<BYTE*>(HeapAlloc(process_heap, 0, 8));
    REQUIRE(data);
    std::memcpy(data, "coinrun", 8);
    data = static_cast<BYTE*>(API(heap, HeapReAlloc)(process_heap, HEAP_ZERO_MEMORY, data, 128));
    REQUIRE(data && !std::memcmp(data, "coinrun", 8) && data[127] == 0);
    REQUIRE(HeapFree(process_heap, 0, data));
    using Calloc = void* (*)(size_t, size_t);
    auto words = static_cast<unsigned*>(crt.function<Calloc>("_calloc_base")(8, sizeof(unsigned)));
    REQUIRE(words && words[0] == 0 && words[7] == 0);
    crt.function<void (*)(void*)>("_free_base")(words);
    auto aligned = API(crt, _aligned_malloc)(80, 64);
    REQUIRE(aligned && reinterpret_cast<uintptr_t>(aligned) % 64 == 0);
    auto offset = API(crt, _aligned_offset_malloc)(80, 64, 8);
    REQUIRE(offset && (reinterpret_cast<uintptr_t>(offset) + 8) % 64 == 0);
    std::memset(aligned, 7, 80);
    std::memset(offset, 9, 80);
    REQUIRE(static_cast<BYTE*>(aligned)[79] == 7 && static_cast<BYTE*>(offset)[79] == 9);
    API(crt, _aligned_free)(aligned);
    API(crt, _aligned_free)(offset);
    auto global = GlobalAlloc(GPTR, 16);
    REQUIRE(global && API(kernel, GlobalFree)(global) == nullptr);
}

void file_io() {
    Module kernel("kernel32.dll"), file("api-ms-win-core-file-l1-1-0.dll"), handles("api-ms-win-core-handle-l1-1-0.dll");
    constexpr DWORD share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    // The launcher runs the probe in a new private directory.
    HANDLE writer = API(kernel, CreateFileW)(L"probe.bin", GENERIC_READ | GENERIC_WRITE, share, nullptr,
                                              CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
    REQUIRE(writer != INVALID_HANDLE_VALUE);
    const char payload[] = {'C', '\0', 'i', 'n', '!'};
    DWORD size = 0;
    REQUIRE(API(kernel, WriteFile)(writer, payload, sizeof(payload), &size, nullptr) && size == sizeof(payload));
    BY_HANDLE_FILE_INFORMATION info{};
    REQUIRE(API(file, GetFileInformationByHandle)(writer, &info) && info.nFileSizeLow == 5 && !info.nFileSizeHigh);
    const DWORD attr = API(kernel, GetFileAttributesW)(L"probe.bin");
    REQUIRE(attr != INVALID_FILE_ATTRIBUTES && !(attr & FILE_ATTRIBUTE_DIRECTORY));
    REQUIRE(API(file, GetFileAttributesW)(L"probe.bin") == attr);
    HANDLE process = API(kernel, GetCurrentProcess)(), reader = nullptr;
    REQUIRE(GetProcessId(process) == API(kernel, GetCurrentProcessId)());
    REQUIRE(API(handles, DuplicateHandle)(process, writer, process, &reader, 0, FALSE, DUPLICATE_SAME_ACCESS));
    REQUIRE(API(kernel, CloseHandle)(writer));
    LARGE_INTEGER position{}, actual{};
    position.QuadPart = 1;
    REQUIRE(API(file, SetFilePointerEx)(reader, position, &actual, FILE_BEGIN) && actual.QuadPart == 1);
    char result[5]{};
    REQUIRE(API(kernel, ReadFile)(reader, result, 4, &size, nullptr) && size == 4 && !std::memcmp(result, payload + 1, 4));
    REQUIRE(API(kernel, CloseHandle)(reader));
    reader = API(kernel, CreateFileA)("probe.bin", GENERIC_READ, share, nullptr, OPEN_EXISTING, 0, nullptr);
    REQUIRE(reader != INVALID_HANDLE_VALUE && API(kernel, SetFilePointer)(reader, -1, nullptr, FILE_END) == 4);
    REQUIRE(API(kernel, ReadFile)(reader, result, 1, &size, nullptr) && size == 1 && result[0] == '!');
    REQUIRE(API(kernel, CloseHandle)(reader) && DeleteFileW(L"probe.bin"));
    auto last_error = API(kernel, GetLastError);
    REQUIRE(API(kernel, GetFileAttributesW)(L"probe.bin") == INVALID_FILE_ATTRIBUTES);
    REQUIRE(last_error() == ERROR_FILE_NOT_FOUND);
}

void crt_file_io() {
    Module io("api-ms-win-crt-stdio-l1-1-0.dll"), fs("api-ms-win-crt-filesystem-l1-1-0.dll");
    Module env("api-ms-win-crt-environment-l1-1-0.dll");
    wchar_t original[MAX_PATH];
    REQUIRE(API(env, _wgetcwd)(original, MAX_PATH) == original);
    REQUIRE(CreateDirectoryW(L"streams", nullptr) && API(fs, _wchdir)(L"streams") == 0);
    int fd = API(io, _wopen)(L"data.bin", _O_CREAT | _O_EXCL | _O_BINARY | _O_RDWR, _S_IREAD | _S_IWRITE);
    REQUIRE(fd >= 0);
    const char payload[] = {'A', '\0', 'Z'};
    REQUIRE(API(io, _write)(fd, payload, 3) == 3);
    REQUIRE(API(io, _lseeki64)(fd, -2, SEEK_END) == 1);
    char got[4]{};
    REQUIRE(API(io, _read)(fd, got, 2) == 2 && !std::memcmp(got, payload + 1, 2));
    REQUIRE(API(io, _close)(fd) == 0);
    REQUIRE(API(fs, _wrename)(L"data.bin", L"renamed.bin") == 0);
    fd = API(io, _open)("renamed.bin", _O_BINARY | _O_RDONLY, 0);
    REQUIRE(fd >= 0 && API(io, _read)(fd, got, 3) == 3 && !std::memcmp(got, payload, 3));
    REQUIRE(API(io, _close)(fd) == 0);

    // The API-set FILE* belongs to its UCRT. Close it through that same provider.
    FILE* stream = API(io, _fsopen)("text.txt", "w+b", _SH_DENYNO);
    REQUIRE(stream && API(io, fputs)("coin", stream) >= 0 && API(io, _ftelli64)(stream) == 4);
    REQUIRE(API(io, fclose)(stream) == 0);
    stream = API(io, _fsopen)("text.txt", "r", _SH_DENYNO);
    REQUIRE(stream && API(io, fgetwc)(stream) == L'c');
    REQUIRE(API(io, ungetwc)(L'X', stream) == L'X' && API(io, fgetwc)(stream) == L'X');
    REQUIRE(API(io, fgetwc)(stream) == L'o' && API(io, fclose)(stream) == 0);
    stream = API(io, _fsopen)("wide.txt", "w, ccs=UTF-8", _SH_DENYNO);
    REQUIRE(stream && API(io, fputwc)(L'\u03a9', stream) == L'\u03a9' && API(io, fclose)(stream) == 0);
    stream = API(io, _fsopen)("wide.txt", "r, ccs=UTF-8", _SH_DENYNO);
    REQUIRE(stream && API(io, fgetwc)(stream) == L'\u03a9' && API(io, fclose)(stream) == 0);
    for (const auto* name : {L"renamed.bin", L"text.txt", L"wide.txt"})
        REQUIRE(API(fs, _wremove)(name) == 0);
    REQUIRE(API(fs, _wchdir)(original) == 0 && API(fs, _wrmdir)(L"streams") == 0);
}

void slist_order() {
    Module core("api-ms-win-core-interlocked-l1-1-0.dll"), kernel("kernel32.dll");
    alignas(MEMORY_ALLOCATION_ALIGNMENT) SLIST_HEADER head{};
    struct alignas(MEMORY_ALLOCATION_ALIGNMENT) Item { SLIST_ENTRY entry; };
    Item items[3]{};
    API(kernel, InitializeSListHead)(&head);
    REQUIRE(API(core, QueryDepthSList)(&head) == 0);
    REQUIRE(API(core, InterlockedPushEntrySList)(&head, &items[0].entry) == nullptr);
    REQUIRE(API(core, InterlockedPushEntrySList)(&head, &items[1].entry) == &items[0].entry);
    REQUIRE(API(core, InterlockedPushEntrySList)(&head, &items[2].entry) == &items[1].entry);
    REQUIRE(API(core, QueryDepthSList)(&head) == 3 && API(core, InterlockedPopEntrySList)(&head) == &items[2].entry);
    auto* chain = API(core, InterlockedFlushSList)(&head);
    REQUIRE(chain == &items[1].entry && chain->Next == &items[0].entry && !chain->Next->Next);
    REQUIRE(API(core, QueryDepthSList)(&head) == 0 && !API(core, InterlockedPopEntrySList)(&head));
}

void fiber_storage() {
    Module fls("api-ms-win-core-fibers-l1-1-0.dll"), util("api-ms-win-core-util-l1-1-0.dll");
    LONG calls = 0;
    DWORD index = API(fls, FlsAlloc)([](void* value) { InterlockedIncrement(static_cast<LONG*>(value)); });
    REQUIRE(index != FLS_OUT_OF_INDEXES && !API(fls, FlsGetValue)(index));
    void* encoded = API(util, EncodePointer)(&calls);
    REQUIRE(API(fls, FlsSetValue)(index, API(util, DecodePointer)(encoded)));
    REQUIRE(API(fls, FlsGetValue)(index) == &calls);
    REQUIRE(API(fls, FlsFree)(index) && calls == 1);
}

void environment() {
    Module proc("api-ms-win-core-processenvironment-l1-1-0.dll"), crt("api-ms-win-crt-environment-l1-1-0.dll");
    const char* name = "TOMBOX_IMPORT_PROBE_VALUE";
    char buffer[32]{};
    REQUIRE(API(proc, SetEnvironmentVariableA)(name, "win32"));
    REQUIRE(API(proc, GetEnvironmentVariableA)(name, buffer, sizeof(buffer)) == 5 && !std::strcmp(buffer, "win32"));
    REQUIRE(API(crt, _putenv_s)(name, "crt") == 0);
    REQUIRE(API(proc, GetEnvironmentVariableA)(name, buffer, sizeof(buffer)) == 3 && !std::strcmp(buffer, "crt"));
    REQUIRE(API(crt, _putenv_s)(name, "") == 0 && API(proc, GetEnvironmentVariableA)(name, buffer, sizeof(buffer)) == 0);
}

void events_and_semaphores() {
    Module kernel("kernel32.dll"), sync("api-ms-win-core-synch-l1-1-0.dll"), waits("api-ms-win-core-synch-l1-2-1.dll");
    HANDLE event = API(kernel, CreateEventW)(nullptr, TRUE, FALSE, nullptr);
    HANDLE semaphore = API(sync, CreateSemaphoreExW)(nullptr, 0, 2, nullptr, 0, SEMAPHORE_ALL_ACCESS);
    REQUIRE(event && semaphore);
    HANDLE handles[] = {event, semaphore};
    REQUIRE(API(waits, WaitForMultipleObjects)(2, handles, FALSE, 0) == WAIT_TIMEOUT);
    LONG before = -1;
    REQUIRE(API(sync, ReleaseSemaphore)(semaphore, 2, &before) && before == 0);
    REQUIRE(API(sync, WaitForMultipleObjectsEx)(2, handles, FALSE, 0, FALSE) == WAIT_OBJECT_0 + 1);
    REQUIRE(API(kernel, SetEvent)(event) && API(waits, WaitForMultipleObjects)(2, handles, TRUE, 0) == WAIT_OBJECT_0);
    REQUIRE(API(kernel, WaitForSingleObject)(semaphore, 0) == WAIT_TIMEOUT);
    REQUIRE(API(kernel, WaitForSingleObject)(event, 0) == WAIT_OBJECT_0);
    REQUIRE(API(kernel, CloseHandle)(event) && API(kernel, CloseHandle)(semaphore));
}

void locks() {
    Module kernel("kernel32.dll"), sync("api-ms-win-core-synch-l1-1-0.dll");
    CRITICAL_SECTION cs{};
    REQUIRE(API(sync, InitializeCriticalSectionEx)(&cs, 0, 0));
    auto available_to_worker = [&] {
        bool acquired = false;
        std::thread worker([&] {
            acquired = TryEnterCriticalSection(&cs) != FALSE;
            if (acquired) LeaveCriticalSection(&cs);
        });
        worker.join();
        return acquired;
    };
    API(kernel, EnterCriticalSection)(&cs);
    API(kernel, EnterCriticalSection)(&cs);
    API(kernel, LeaveCriticalSection)(&cs);
    REQUIRE(!available_to_worker());
    API(kernel, LeaveCriticalSection)(&cs);
    REQUIRE(available_to_worker());
    API(kernel, DeleteCriticalSection)(&cs);
    API(kernel, InitializeCriticalSection)(&cs);
    REQUIRE(TryEnterCriticalSection(&cs));
    API(kernel, LeaveCriticalSection)(&cs);
    API(kernel, DeleteCriticalSection)(&cs);

    SRWLOCK lock = SRWLOCK_INIT;
    API(kernel, AcquireSRWLockExclusive)(&lock);
    auto shared = API(sync, TryAcquireSRWLockShared);
    std::thread([&] { REQUIRE(!shared(&lock)); }).join();
    API(kernel, ReleaseSRWLockExclusive)(&lock);
    REQUIRE(API(sync, TryAcquireSRWLockShared)(&lock));
    auto exclusive = API(sync, TryAcquireSRWLockExclusive);
    std::thread([&] { REQUIRE(!exclusive(&lock)); }).join();
    ReleaseSRWLockShared(&lock);
    REQUIRE(API(sync, TryAcquireSRWLockExclusive)(&lock));
    API(kernel, ReleaseSRWLockExclusive)(&lock);
}

void pool_callbacks() {
    Module pool("api-ms-win-core-threadpool-l1-2-0.dll"), kernel("kernel32.dll");
    struct Context { HANDLE fired; volatile LONG calls; };
    Context ctx{CreateEventW(nullptr, FALSE, FALSE, nullptr), 0};
    REQUIRE(ctx.fired);
    auto work_cb = [](PTP_CALLBACK_INSTANCE, void* raw, PTP_WORK) {
        InterlockedIncrement(&static_cast<Context*>(raw)->calls);
    };
    auto timer_cb = [](PTP_CALLBACK_INSTANCE, void* raw, PTP_TIMER) {
        auto* context = static_cast<Context*>(raw);
        InterlockedIncrement(&context->calls);
        SetEvent(context->fired);
    };
    for (auto* module : {&pool, &kernel}) {
        ctx.calls = 0;
        auto work = API((*module), CreateThreadpoolWork)(work_cb, &ctx, nullptr);
        REQUIRE(work);
        API((*module), SubmitThreadpoolWork)(work);
        API((*module), SubmitThreadpoolWork)(work);
        API((*module), WaitForThreadpoolWorkCallbacks)(work, FALSE);
        REQUIRE(ctx.calls == 2);
        API((*module), CloseThreadpoolWork)(work);
        auto timer = API((*module), CreateThreadpoolTimer)(timer_cb, &ctx, nullptr);
        REQUIRE(timer);
        LARGE_INTEGER due{};
        due.QuadPart = -10000;
        FILETIME stamp{};
        std::memcpy(&stamp, &due, sizeof(stamp));
        API((*module), SetThreadpoolTimer)(timer, &stamp, 0, 0);
        REQUIRE(WaitForSingleObject(ctx.fired, 5000) == WAIT_OBJECT_0);
        API((*module), SetThreadpoolTimer)(timer, nullptr, 0, 0);
        API((*module), WaitForThreadpoolTimerCallbacks)(timer, TRUE);
        REQUIRE(ctx.calls == 3);
        API((*module), CloseThreadpoolTimer)(timer);
    }
    HANDLE signal = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    auto wait_cb = [](PTP_CALLBACK_INSTANCE, void* raw, PTP_WAIT, TP_WAIT_RESULT reason) {
        auto* context = static_cast<Context*>(raw);
        if (reason == WAIT_OBJECT_0) InterlockedIncrement(&context->calls);
        SetEvent(context->fired);
    };
    auto wait = API(pool, CreateThreadpoolWait)(wait_cb, &ctx, nullptr);
    REQUIRE(signal && wait);
    API(pool, SetThreadpoolWait)(wait, signal, nullptr);
    REQUIRE(SetEvent(signal) && WaitForSingleObject(ctx.fired, 5000) == WAIT_OBJECT_0);
    API(pool, SetThreadpoolWait)(wait, nullptr, nullptr);
    API(pool, WaitForThreadpoolWaitCallbacks)(wait, TRUE);
    REQUIRE(ctx.calls == 4);
    API(pool, CloseThreadpoolWait)(wait);
    REQUIRE(CloseHandle(signal) && CloseHandle(ctx.fired));
}

void runtime_modules() {
    Module kernel("kernel32.dll"), loader("api-ms-win-core-libraryloader-l1-2-0.dll");
    Module rtl("api-ms-win-core-rtlsupport-l1-1-0.dll");
    // The ordinal is an independent cross-check for the name returned by the loader.
    auto first = API(kernel, LoadLibraryW)(L"oleaut32.dll");
    auto second = API(loader, LoadLibraryExW)(L"oleaut32.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    auto third = API(kernel, LoadLibraryExW)(L"oleaut32.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    auto fourth = API(kernel, LoadLibraryExA)("oleaut32.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    REQUIRE(first && second == first && third == first && fourth == first);
    REQUIRE(API(kernel, GetModuleHandleW)(L"oleaut32.dll") == first);
    auto name = API(kernel, GetProcAddress)(first, "SysAllocStringLen");
    REQUIRE(name && name == API(kernel, GetProcAddress)(second, MAKEINTRESOURCEA(4)));
    void* base = nullptr;
    REQUIRE(API(rtl, RtlPcToFileHeader)(reinterpret_cast<void*>(name), &base) == first && base == first);
    auto version = API(loader, FindResourceExW)(first, MAKEINTRESOURCEW(16), MAKEINTRESOURCEW(1), 0);
    REQUIRE(version && SizeofResource(first, version) >= 3 * sizeof(WORD) + sizeof(L"VS_VERSION_INFO"));
    auto resource = API(loader, LoadResource)(first, version);
    auto bytes = static_cast<const BYTE*>(LockResource(resource));
    REQUIRE(bytes && !std::memcmp(bytes + 3 * sizeof(WORD), L"VS_VERSION_INFO", sizeof(L"VS_VERSION_INFO")));
    for (auto handle : {first, second, third, fourth}) REQUIRE(API(kernel, FreeLibrary)(handle));
}

void clocks_and_topology() {
    Module kernel("kernel32.dll"), sys("api-ms-win-core-sysinfo-l1-1-0.dll");
    Module precise("api-ms-win-core-sysinfo-l1-2-0.dll");
    SYSTEM_INFO info{};
    API(precise, GetNativeSystemInfo)(&info);
    REQUIRE(info.dwPageSize >= 4096 && (info.dwPageSize & (info.dwPageSize - 1)) == 0);
    REQUIRE(info.dwAllocationGranularity >= info.dwPageSize && info.dwNumberOfProcessors > 0);
    DWORD size = 0;
    auto topology = API(sys, GetLogicalProcessorInformationEx);
    REQUIRE(!topology(RelationProcessorCore, nullptr, &size) && GetLastError() == ERROR_INSUFFICIENT_BUFFER && size);
    std::vector<BYTE> bytes(size);
    REQUIRE(topology(RelationProcessorCore, reinterpret_cast<PSYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(bytes.data()), &size));
    size_t pos = 0;
    while (pos < size) {
        auto* rec = reinterpret_cast<PSYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(bytes.data() + pos);
        REQUIRE(rec->Relationship == RelationProcessorCore && rec->Size >= offsetof(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Processor.GroupMask) + sizeof(GROUP_AFFINITY));
        REQUIRE(rec->Size <= size - pos && rec->Processor.GroupCount && rec->Processor.GroupMask[0].Mask);
        pos += rec->Size;
    }
    auto filetime = [](FILETIME f) { return (UINT64(f.dwHighDateTime) << 32) | f.dwLowDateTime; };
    FILETIME coarse{}, fine{};
    API(kernel, GetSystemTimeAsFileTime)(&coarse);
    API(precise, GetSystemTimePreciseAsFileTime)(&fine);
    REQUIRE(filetime(coarse) > 0 && std::llabs(static_cast<long long>(filetime(fine)) - static_cast<long long>(filetime(coarse))) < 20000000);
    auto ticks = API(sys, GetTickCount64);
    auto low = API(sys, GetTickCount);
    const auto begin = ticks();
    LARGE_INTEGER q0{}, q1{};
    REQUIRE(API(kernel, QueryPerformanceCounter)(&q0));
    Sleep(40);
    REQUIRE(API(kernel, QueryPerformanceCounter)(&q1) && q1.QuadPart > q0.QuadPart);
    DWORD low_tick = low();
    const auto finish = ticks();
    REQUIRE(finish > begin && low_tick - DWORD(begin) <= finish - begin);
    REQUIRE(API(kernel, GetCurrentThreadId)() == GetThreadId(GetCurrentThread()));
}

void temp_files() {
    Module core("api-ms-win-core-file-l1-1-0.dll"), temp("api-ms-win-core-file-l1-2-0.dll");
    Module copy("api-ms-win-core-file-l2-1-0.dll");
    wchar_t path[MAX_PATH];
    const DWORD length = API(temp, GetTempPathW)(MAX_PATH, path);
    REQUIRE(length && length < MAX_PATH && std::wcslen(path) == length && path[length - 1] == L'\\');
    ULARGE_INTEGER available{}, capacity{}, free{};
    REQUIRE(API(core, GetDiskFreeSpaceExW)(path, &available, &capacity, &free));
    REQUIRE(capacity.QuadPart && available.QuadPart <= free.QuadPart && free.QuadPart <= capacity.QuadPart);
    auto make = [](const wchar_t* name, DWORD disposition, DWORD access) {
        return CreateFileW(name, access, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, disposition, FILE_ATTRIBUTE_NORMAL, nullptr);
    };
    HANDLE src = make(L"original.tmp", CREATE_NEW, GENERIC_WRITE);
    DWORD n = 0;
    REQUIRE(src != INVALID_HANDLE_VALUE && WriteFile(src, "first", 5, &n, nullptr) && n == 5 && CloseHandle(src));
    REQUIRE(SUCCEEDED(API(copy, CopyFile2)(L"original.tmp", L"copy.tmp", nullptr)));
    REQUIRE(API(copy, CreateHardLinkW)(L"linked.tmp", L"original.tmp", nullptr));
    src = make(L"original.tmp", OPEN_EXISTING, GENERIC_WRITE);
    REQUIRE(src != INVALID_HANDLE_VALUE && WriteFile(src, "other", 5, &n, nullptr) && n == 5 && CloseHandle(src));
    auto read = [&](const wchar_t* name, const char* expected) {
        HANDLE input = make(name, OPEN_EXISTING, GENERIC_READ);
        char data[6]{};
        REQUIRE(input != INVALID_HANDLE_VALUE && ReadFile(input, data, sizeof(data), &n, nullptr) && n == 5);
        REQUIRE(std::strcmp(data, expected) == 0 && CloseHandle(input));
    };
    read(L"linked.tmp", "other");
    read(L"copy.tmp", "first");
    for (auto* name : {L"original.tmp", L"linked.tmp", L"copy.tmp"}) REQUIRE(DeleteFileW(name));
}

void apartments() {
    Module rt("api-ms-win-core-winrt-l1-1-0.dll"), com("api-ms-win-core-com-l1-1-0.dll");
    REQUIRE(SUCCEEDED(API(rt, RoInitialize)(RO_INIT_MULTITHREADED)));
    APTTYPE type{};
    APTTYPEQUALIFIER qualifier{};
    REQUIRE(SUCCEEDED(API(com, CoGetApartmentType)(&type, &qualifier)) && type == APTTYPE_MTA);
    IComThreadingInfo* context = nullptr;
    REQUIRE(SUCCEEDED(API(com, CoGetObjectContext)(__uuidof(IComThreadingInfo), reinterpret_cast<void**>(&context))) && context);
    REQUIRE(SUCCEEDED(context->GetCurrentApartmentType(&type)) && type == APTTYPE_MTA);
    context->Release();
    UINT64 id = 0, again = 0;
    REQUIRE(SUCCEEDED(API(rt, RoGetApartmentIdentifier)(&id)));
    REQUIRE(SUCCEEDED(API(rt, RoGetApartmentIdentifier)(&again)) && again == id);
    ULONG_PTR token = 0;
    REQUIRE(SUCCEEDED(API(com, CoGetContextToken)(&token)) && token);
    API(rt, RoUninitialize)();
    REQUIRE(API(com, CoGetApartmentType)(&type, &qualifier) == CO_E_NOTINITIALIZED);
}

void registry_values() {
    Module adv("advapi32.dll"), api("api-ms-win-core-registry-l1-1-0.dll");
    const wchar_t* path = L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    wchar_t expected[MAX_PATH];
    const UINT len = GetWindowsDirectoryW(expected, MAX_PATH);
    REQUIRE(len && len < MAX_PATH);
    HKEY key{};
    for (auto* module : {&adv, &api}) {
        REQUIRE(API((*module), RegOpenKeyExW)(HKEY_LOCAL_MACHINE, path, 0, KEY_QUERY_VALUE, &key) == ERROR_SUCCESS);
        wchar_t value[MAX_PATH]{};
        DWORD type = 0, size = sizeof(value);
        REQUIRE(API(adv, RegQueryValueExW)(key, L"SystemRoot", nullptr, &type, reinterpret_cast<BYTE*>(value), &size) == ERROR_SUCCESS);
        REQUIRE(type == REG_SZ && size == (len + 1) * sizeof(wchar_t) && !_wcsicmp(value, expected));
        REQUIRE(API(adv, RegCloseKey)(key) == ERROR_SUCCESS);
    }
    REQUIRE(API(adv, RegOpenKeyW)(HKEY_LOCAL_MACHINE, path, &key) == ERROR_SUCCESS);
    REQUIRE(API(adv, RegCloseKey)(key) == ERROR_SUCCESS);
}

void condition_variables() {
    Module kernel("kernel32.dll"), sync("api-ms-win-core-synch-l1-2-0.dll");
    INIT_ONCE once = INIT_ONCE_STATIC_INIT;
    unsigned initialized = 0;
    auto initialize = [](PINIT_ONCE, void* context, void**) -> BOOL {
        ++*static_cast<unsigned*>(context);
        return TRUE;
    };
    for (int i = 0; i < 3; ++i) REQUIRE(API(sync, InitOnceExecuteOnce)(&once, initialize, &initialized, nullptr));
    REQUIRE(initialized == 1);
    CONDITION_VARIABLE cv{};
    API(sync, InitializeConditionVariable)(&cv);
    // Each waiter signals readiness while holding the lock, then atomically releases it to sleep.
    HANDLE ready = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    REQUIRE(ready);
    CRITICAL_SECTION cs{};
    InitializeCriticalSection(&cs);
    bool go = false;
    auto sleep_cs = API(sync, SleepConditionVariableCS);
    std::thread sleeper([&] {
        EnterCriticalSection(&cs);
        REQUIRE(SetEvent(ready));
        while (!go) REQUIRE(sleep_cs(&cv, &cs, 5000));
        LeaveCriticalSection(&cs);
    });
    REQUIRE(WaitForSingleObject(ready, 5000) == WAIT_OBJECT_0);
    EnterCriticalSection(&cs);
    go = true;
    API(sync, WakeConditionVariable)(&cv);
    LeaveCriticalSection(&cs);
    sleeper.join();
    DeleteCriticalSection(&cs);
    SRWLOCK lock = SRWLOCK_INIT;
    for (auto* module : {&kernel, &sync}) {
        go = false;
        auto sleep_srw = API((*module), SleepConditionVariableSRW);
        auto wait_shared = [&] {
            AcquireSRWLockShared(&lock);
            REQUIRE(SetEvent(ready));
            while (!go) REQUIRE(sleep_srw(&cv, &lock, 5000, CONDITION_VARIABLE_LOCKMODE_SHARED));
            ReleaseSRWLockShared(&lock);
        };
        std::thread first(wait_shared);
        REQUIRE(WaitForSingleObject(ready, 5000) == WAIT_OBJECT_0);
        std::thread second(wait_shared);
        REQUIRE(WaitForSingleObject(ready, 5000) == WAIT_OBJECT_0);
        AcquireSRWLockExclusive(&lock);
        go = true;
        API((*module), WakeAllConditionVariable)(&cv);
        ReleaseSRWLockExclusive(&lock);
        first.join();
        second.join();
    }
    REQUIRE(CloseHandle(ready));
}

void errors_and_messages() {
    Module loc("api-ms-win-core-localization-l1-2-0.dll"), crt("api-ms-win-crt-runtime-l1-1-0.dll");
    char message[80]{};
    DWORD len = API(loc, FormatMessageA)(FORMAT_MESSAGE_FROM_STRING | FORMAT_MESSAGE_IGNORE_INSERTS,
                                        "Coin Run%0", 0, 0, message, sizeof(message), nullptr);
    REQUIRE(len == 8 && !std::strcmp(message, "Coin Run"));
    // The CRT's errno storage and error strings are owned by the same UCRT provider.
    auto errno_ptr = crt.function<int* (*)()>("_errno");
    const int saved = *errno_ptr();
    REQUIRE(API(crt, _set_errno)(ENOENT) == 0 && *errno_ptr() == ENOENT);
    std::string absent = API(crt, strerror)(ENOENT);
    std::string denied = API(crt, strerror)(EACCES);
    REQUIRE(!absent.empty() && !denied.empty() && absent != denied);
    REQUIRE(API(crt, _set_errno)(saved) == 0);
    Module crypt("crypt32.dll");
    BYTE decoded[8]{};
    DWORD bytes = 0;
    auto from_base64 = API(crypt, CryptStringToBinaryW);
    REQUIRE(from_base64(L"QQBC/w==", 0, CRYPT_STRING_BASE64, nullptr, &bytes, nullptr, nullptr) && bytes == 4);
    REQUIRE(from_base64(L"QQBC/w==", 0, CRYPT_STRING_BASE64, decoded, &bytes, nullptr, nullptr));
    REQUIRE(bytes == 4 && !std::memcmp(decoded, "A\0B\xff", 4));
}

void com_stream() {
    Module com("api-ms-win-core-com-l1-1-0.dll");
    REQUIRE(SUCCEEDED(CoInitializeEx(nullptr, COINIT_MULTITHREADED)));
    ULONG refs = API(com, CoAddRefServerProcess)();
    REQUIRE(refs > 0 && API(com, CoReleaseServerProcess)() == refs - 1);
    IStream *stream = nullptr, *packet = nullptr, *copy = nullptr;
    REQUIRE(SUCCEEDED(CreateStreamOnHGlobal(nullptr, TRUE, &stream)) && stream);
    ULONG size = 0;
    REQUIRE(SUCCEEDED(stream->Write("coin", 4, &size)) && size == 4);
    REQUIRE(SUCCEEDED(stream->Seek({}, STREAM_SEEK_SET, nullptr)));
    REQUIRE(SUCCEEDED(API(com, CoMarshalInterThreadInterfaceInStream)(__uuidof(IStream), stream, &packet)) && packet);
    auto receive = API(com, CoGetInterfaceAndReleaseStream);
    std::thread receiver([&] {
        REQUIRE(SUCCEEDED(CoInitializeEx(nullptr, COINIT_MULTITHREADED)));
        REQUIRE(SUCCEEDED(receive(packet, __uuidof(IStream), reinterpret_cast<void**>(&copy))) && copy);
        char data[4]{};
        ULONG read = 0;
        REQUIRE(SUCCEEDED(copy->Read(data, 4, &read)) && read == 4 && !std::memcmp(data, "coin", 4));
        copy->Release();
        CoUninitialize();
    });
    receiver.join();
    stream->Release();
    CoUninitialize();
}

void crt_locale() {
    Module crt("api-ms-win-crt-locale-l1-1-0.dll"), conv("api-ms-win-crt-string-l1-1-0.dll");
    auto locale = API(crt, setlocale);
    REQUIRE(locale(LC_ALL, "C") && !std::strcmp(locale(LC_NUMERIC, nullptr), "C"));
    auto values = API(crt, localeconv)();
    REQUIRE(values && !std::strcmp(values->decimal_point, ".") && !values->thousands_sep[0]);
    REQUIRE(crt.function<int (*)()>("___mb_cur_max_func")() == 1);
    const auto table = crt.function<const unsigned short* (*)()>("__pctype_func")();
    REQUIRE(table && (table['0'] & _DIGIT) && (table['a'] & _LOWER) && !(table['!'] & _LOWER));
    auto dup = API(conv, _wcsdup)(L"Coin");
    REQUIRE(dup && !std::wcscmp(dup, L"Coin"));
    dup[0] = L'J';
    REQUIRE(!std::wcscmp(dup, L"Join"));
    Module heap("api-ms-win-crt-heap-l1-1-0.dll");
    API(heap, free)(dup);
}

void url_parts() {
    Module http("winhttp.dll");
    URL_COMPONENTS parts{};
    parts.dwStructSize = sizeof(parts);
    parts.dwHostNameLength = parts.dwUrlPathLength = parts.dwExtraInfoLength = DWORD(-1);
    REQUIRE(API(http, WinHttpCrackUrl)(L"https://example.invalid:444/a%20b?q=3", 0, 0, &parts));
    REQUIRE(parts.nScheme == INTERNET_SCHEME_HTTPS && parts.nPort == 444);
    REQUIRE(std::wstring(parts.lpszHostName, parts.dwHostNameLength) == L"example.invalid");
    REQUIRE(std::wstring(parts.lpszUrlPath, parts.dwUrlPathLength) == L"/a%20b");
    REQUIRE(std::wstring(parts.lpszExtraInfo, parts.dwExtraInfoLength) == L"?q=3");
}
} // namespace

int main() {
    struct Scenario { const char* name; void (*run)(); };
    const Scenario scenarios[] = {
        {"length-prefixed BSTR with embedded NUL", automation_strings},
        {"owned and referenced WinRT strings with embedded NUL", winrt_strings},
        {"UTF-8 round trip, casing and character classification", unicode_text},
        {"numeric conversions and signed floating-point reference values", numbers},
        {"CRT wide argument retrieval and LIFO exit callbacks", runtime_callbacks},
        {"suspended thread priority, worker exit code and queued event", thread_lifecycle},
        {"bounded narrow and wide strings and character predicates", crt_strings},
        {"zeroed, resized and aligned allocations with paired release", allocations},
        {"file contents, offsets and duplicate-handle lifetime", file_io},
        {"CRT file offsets, UTF-8 streams, rename and removal", crt_file_io},
        {"interlocked list depth, pop and flush order", slist_order},
        {"fiber storage pointer round trip and cleanup callback", fiber_storage},
        {"Win32 and CRT process environment round trip", environment},
        {"event and semaphore counts with single and multiple waits", events_and_semaphores},
        {"critical section recursion and exclusive/shared lock ownership", locks},
        {"thread pool work, one-shot timer and signaled wait callbacks", pool_callbacks},
        {"loader handles, ordinal address, image ownership and version resource", runtime_modules},
        {"processor core topology and monotonic and system clocks", clocks_and_topology},
        {"temporary directory, hard-link visibility and independent file copy", temp_files},
        {"WinRT MTA initialization, context and balanced teardown", apartments},
        {"Windows directory registry value through both open-key contracts", registry_values},
        {"one-time initialization and condition variable wakeups", condition_variables},
        {"formatted messages, CRT error state and binary Base64 decoding", errors_and_messages},
        {"COM server references and cross-thread stream readback", com_stream},
        {"C locale numeric formatting, classification and string duplication", crt_locale},
        {"offline HTTPS URL component parsing", url_parts},
    };
    for (auto& test : scenarios) {
        scenario = test.name;
        test.run();
    }
    std::puts("dependency\tsymbol\tevidence");
    for (const auto& [key, evidence] : checked)
        std::printf("%s\t%s\t%s\n", key.first.c_str(), key.second.c_str(), evidence.c_str());
    return std::fflush(stdout) != 0 || std::ferror(stdout) != 0;
}
