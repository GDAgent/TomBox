// Behavior checks for exact CRT imports from both the recorded title and Scarlett DLL audit.
#define _CRT_SECURE_NO_WARNINGS
#define NOMINMAX
#include "import_probe.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <cwchar>
#include <ctime>
#include <cstdint>
#include <limits>
#include <locale.h>
#include <new.h>
#include <io.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <corecrt_startup.h>
#include <xlocinfo>
#include <xthreads.h>
#include <signal.h>
#include <cassert>
#include <process.h>

namespace {
void conversion() {
    Module m("api-ms-win-crt-convert-l1-1-0.dll");
    const char text[] = "-100000001!";
    char *end = nullptr;
    for (auto f : {API(m, _strtoi64), API(m, strtoll)}) {
        REQUIRE(f(text, &end, 16) == -0x100000001LL && end == text + 10);
    }
    wchar_t *wend = nullptr;
    const wchar_t wide[] = L"-100000001!";
    REQUIRE(API(m, _wcstoi64)(wide, &wend, 16) == -0x100000001LL && wend == wide + 10);
    REQUIRE(API(m, _wcstoui64)(wide + 1, &wend, 16) == 0x100000001ULL && wend == wide + 10);
    REQUIRE(API(m, _wtoi64)(L" -4294967297x") == -4294967297LL);
    REQUIRE(API(m, atof)(" -12.5e1x") == -125.0 && API(m, _wtof)(L" 1.25e2x") == 125.0);
    const wchar_t decimal[] = L"-.125e1!";
    REQUIRE(API(m, wcstod)(decimal, &wend) == -1.25 && wend == decimal + 7);
}

void arithmetic() {
    Module m("api-ms-win-crt-math-l1-1-0.dll");
    const double pi = 3.141592653589793;
    struct Case {
        const char *name;
        double input, output;
    };
    for (auto c : {Case{"acos", 0, pi / 2},
                   {"asin", -1, -pi / 2},
                   {"log10", 1000, 3},
                   {"exp2", -3, .125},
                   {"round", -2.5, -3},
                   {"trunc", -2.9, -2}}) {
        auto f = m.function<double (*)(double)>(c.name);
        REQUIRE(std::abs(f(c.input) - c.output) < 1e-12);
    }
    for (auto c : {Case{"acosf", 0, pi / 2},
                   {"asinf", -1, -pi / 2},
                   {"atanf", 1, pi / 4},
                   {"cosf", pi, -1},
                   {"expf", 1, 2.718281828},
                   {"floorf", -2.25, -3},
                   {"log10f", 100, 2},
                   {"roundf", 2.5, 3},
                   {"sinf", pi / 2, 1},
                   {"sinhf", 1, 1.175201194},
                   {"tanf", pi / 4, 1},
                   {"truncf", -2.9, -2}}) {
        auto f = m.function<float (*)(float)>(c.name);
        REQUIRE(std::abs(f(static_cast<float>(c.input)) - c.output) < 1e-5);
    }
    auto quadrant = m.function<double (*)(double, double)>("atan2");
    auto quadrantf = m.function<float (*)(float, float)>("atan2f");
    REQUIRE(std::abs(quadrant(1, -1) - 3 * pi / 4) < 1e-12 && std::abs(quadrantf(-1, -1) + 3 * pi / 4) < 1e-5);
    REQUIRE(m.function<float (*)(float, float)>("fmodf")(-5.5f, 2) == -1.5f);
    REQUIRE(m.function<long double (*)(long double, long double)>("fminl")(4, -2) == -2);
    double i;
    float j;
    REQUIRE(m.function<double (*)(double, double *)>("modf")(-2.75, &i) == -.75 && i == -2);
    REQUIRE(m.function<float (*)(float, float *)>("modff")(3.25f, &j) == .25f && j == 3);
    auto finite = API(m, _finite);
    auto classify = m.function<short (*)(double)>("_dclass");
    auto test = m.function<short (*)(double *)>("_dtest");
    for (double x : {0., 1., std::numeric_limits<double>::infinity(), std::numeric_limits<double>::quiet_NaN()}) {
        REQUIRE(classify(x) == std::fpclassify(x) && test(&x) == std::fpclassify(x));
        REQUIRE(bool(finite(x)) == std::isfinite(x));
    }
}

void strings() {
    Module m("api-ms-win-crt-string-l1-1-0.dll");
    using Pred = int (*)(int);
    struct Category {
        const char *name;
        int yes, no;
    };
    for (auto c : {Category{"isalpha", 'A', '1'}, {"isdigit", '7', 'a'}, {"isspace", '\t', 'x'}}) {
        auto f = m.function<Pred>(c.name);
        REQUIRE(f(c.yes) && !f(c.no));
    }
    for (auto c : {Category{"iswalnum", '7', '-'},
                   {"iswalpha", 'Q', '4'},
                   {"iswdigit", '0', 'X'},
                   {"iswlower", 'x', 'X'},
                   {"iswpunct", '!', '9'},
                   {"iswspace", '\n', 'A'},
                   {"iswupper", 'A', 'a'},
                   {"iswxdigit", 'F', 'G'}}) {
        auto f = m.function<int (*)(wint_t)>(c.name);
        REQUIRE(f(c.yes) && !f(c.no));
    }
    REQUIRE(m.function<Pred>("tolower")('F') == 'f' && m.function<Pred>("toupper")('a') == 'A');
    REQUIRE(API(m, _stricmp)("Coin", "COIN") == 0 && API(m, _strnicmp)("Coin!", "COIN?", 4) == 0);
    REQUIRE(API(m, strcmp)("beta", "alpha") > 0 && API(m, strncmp)("coinX", "coinY", 4) == 0);
    REQUIRE(API(m, wcscmp)(L"beta", L"alpha") > 0 && API(m, wcsncmp)(L"coinX", L"coinY", 4) == 0);
    char buf[12]{};
    wchar_t wide[12]{};
    using Copy = errno_t (*)(char *, size_t, const char *);
    using WCopy = errno_t (*)(wchar_t *, size_t, const wchar_t *);
    REQUIRE(m.function<Copy>("strcpy_s")(buf, 12, "ab:cd") == 0 && !std::strcmp(buf, "ab:cd"));
    REQUIRE(m.function<WCopy>("wcscpy_s")(wide, 12, L"ab:cd") == 0 && !std::wcscmp(wide, L"ab:cd"));
    REQUIRE(API(m, strcspn)(buf, ":") == 2 && API(m, strspn)(buf, "abc") == 2);
    char *ctx = nullptr;
    wchar_t *wctx = nullptr;
    auto tok = API(m, strtok_s);
    auto wtok = m.function<wchar_t *(*)(wchar_t *, const wchar_t *, wchar_t **)>("wcstok_s");
    REQUIRE(tok(buf, ":", &ctx) == buf && !std::strcmp(buf, "ab") && !std::strcmp(tok(nullptr, ":", &ctx), "cd"));
    REQUIRE(wtok(wide, L":", &wctx) == wide && !std::wcscmp(wide, L"ab") &&
            !std::wcscmp(wtok(nullptr, L":", &wctx), L"cd"));
    using NCopy = char *(*)(char *, const char *, size_t);
    using WNCopy = wchar_t *(*)(wchar_t *, const wchar_t *, size_t);
    REQUIRE(m.function<NCopy>("strncpy")(buf, "1234", 3) == buf && !std::memcmp(buf, "123c", 4));
    REQUIRE(API(m, strnlen)(buf, 2) == 2);
    REQUIRE(m.function<WNCopy>("wcsncpy")(wide, L"1234", 3) == wide && !std::wmemcmp(wide, L"123c", 4));
}

void sort_and_random() {
    Module m("api-ms-win-crt-utility-l1-1-0.dll");
    int items[] = {7, -2, 56, 1};
    auto cmp = [](const void *a, const void *b) {
        return *static_cast<const int *>(a) - *static_cast<const int *>(b);
    };
    using Sort = void (*)(void *, size_t, size_t, int (*)(const void *, const void *));
    using Search = void *(*)(const void *, const void *, size_t, size_t, int (*)(const void *, const void *));
    m.function<Sort>("qsort")(items, 4, sizeof(int), cmp);
    REQUIRE(std::is_sorted(items, items + 4));
    int key = 7;
    REQUIRE(m.function<Search>("bsearch")(&key, items, 4, sizeof(int), cmp) == items + 2);
    auto result = m.function<div_t (*)(int, int)>("div")(-17, 5);
    REQUIRE(result.quot == -3 && result.rem == -2);
    auto seed = API(m, srand);
    auto next = API(m, rand);
    seed(17);
    int a = next(), b = next();
    seed(17);
    REQUIRE(a != b && next() == a && next() == b);
    unsigned sample;
    REQUIRE(m.function<errno_t (*)(unsigned *)>("rand_s")(&sample) == 0);
}

void allocation() {
    Module m("api-ms-win-crt-heap-l1-1-0.dll");
    auto alloc = API(m, malloc);
    auto zero = API(m, calloc);
    auto resize = API(m, realloc);
    auto release = API(m, free);
    auto data = static_cast<unsigned char *>(alloc(10));
    REQUIRE(data);
    std::memset(data, 42, 10);
    data = static_cast<unsigned char *>(resize(data, 40));
    REQUIRE(data && data[0] == 42 && data[9] == 42);
    release(data);
    data = static_cast<unsigned char *>(zero(8, 5));
    REQUIRE(data && std::all_of(data, data + 40, [](auto x) { return x == 0; }));
    release(data);
    auto mode = API(m, _set_new_mode);
    int previous = mode(1);
    REQUIRE(mode(previous) == 1);
}

void allocation_callback() {
    Module m("api-ms-win-crt-heap-l1-1-0.dll"), rt("api-ms-win-crt-runtime-l1-1-0.dll");
    static size_t requested;
    auto handler = API(rt, _set_new_handler);
    auto call = API(m, _callnewh);
    _PNH callback = [](size_t n) -> int {
        requested = n;
        return 1;
    };
    auto old = handler(callback);

    int result = call(117);
    handler(old);
    if (result != 1 || requested != 117) {
        std::fprintf(stderr, "Unverified: _callnewh returned %d, callback saw %zu (expected 1 and 117)\n", result,
                     requested);
        checked.erase({rt.name, "_set_new_handler"});
        checked.erase({m.name, "_callnewh"});
    }
}

void calendar() {
    Module t("api-ms-win-crt-time-l1-1-0.dll"), heap("api-ms-win-crt-heap-l1-1-0.dll");
    auto release = API(heap, free);
    __time64_t seconds = 86400;
    tm utc{}, local{};
    REQUIRE(API(t, _gmtime64_s)(&utc, &seconds) == 0 && utc.tm_year == 70 && utc.tm_mon == 0 && utc.tm_mday == 2);
    auto shared = API(t, _gmtime64)(&seconds);
    REQUIRE(shared && shared->tm_wday == 5 && shared->tm_hour == 0);
    Module env("api-ms-win-crt-environment-l1-1-0.dll");
    REQUIRE(API(env, _putenv_s)("TZ", "PST8PDT") == 0);
    API(t, _tzset)();
    REQUIRE(*API(t, __timezone)() == 8 * 3600);
    auto zone = API(t, __tzname)();
    REQUIRE(zone && zone[0] && zone[1]);
    if (std::strcmp(zone[0], "PST") || std::strcmp(zone[1], "PDT")) {
        std::fprintf(stderr, "Unverified: __tzname returned %s/%s for PST8PDT\n", zone[0], zone[1]);
        checked.erase({t.name, "__tzname"});
    }
    REQUIRE(API(t, _localtime64_s)(&local, &seconds) == 0 && local.tm_mday == 1 && local.tm_hour == 16);
    __time64_t now;
    auto current = API(t, _time64)(&now);
    FILETIME ft{};
    GetSystemTimeAsFileTime(&ft);
    ULARGE_INTEGER ticks{};
    ticks.HighPart = ft.dwHighDateTime;
    ticks.LowPart = ft.dwLowDateTime;
    REQUIRE(current == now &&
            std::abs(current - static_cast<__time64_t>(ticks.QuadPart / 10000000 - 11644473600LL)) <= 2);
    auto clock_fn = API(t, clock);
    auto start = clock_fn();
    Sleep(32);
    REQUIRE(clock_fn() > start);
    char *days = API(t, _Getdays)(), *months = API(t, _Getmonths)();
    wchar_t *wdays = API(t, _W_Getdays)(), *wmonths = API(t, _W_Getmonths)();
    REQUIRE(days && months && std::strstr(days, "Fri") && std::strstr(months, "Jan"));
    REQUIRE(wdays && wmonths && std::wcsstr(wdays, L"Fri") && std::wcsstr(wmonths, L"Jan"));
    release(days);
    release(months);
    release(wdays);
    release(wmonths);
    auto names = API(t, _Gettnames)();
    auto wnames = API(t, _W_Gettnames)();
    REQUIRE(names && wnames);
    char text[40]{};
    wchar_t wide[40]{};
    REQUIRE(API(t, _Strftime)(text, 40, "%a %Y-%m-%d", &utc, names) == 14 && !std::strcmp(text, "Fri 1970-01-02"));
    REQUIRE(API(t, _Wcsftime)(wide, 40, L"%a %Y-%m-%d", &utc, wnames) == 14 &&
            !std::wcscmp(wide, L"Fri 1970-01-02"));
    release(names);
    release(wnames);
}

void locale_data() {
    Module m("api-ms-win-crt-locale-l1-1-0.dll");
    auto config = API(m, _configthreadlocale);
    int saved = config(_ENABLE_PER_THREAD_LOCALE);
    REQUIRE(saved != -1 && config(0) == _ENABLE_PER_THREAD_LOCALE);
    auto current = API(m, setlocale);
    REQUIRE(current(LC_ALL, "English_United States.1252"));
    auto lock = m.function<void (*)()>("_lock_locales");
    auto unlock = m.function<void (*)()>("_unlock_locales");
    lock();
    REQUIRE(m.function<unsigned (*)()>("___lc_codepage_func")() == 1252);
    REQUIRE(m.function<unsigned (*)()>("___lc_collate_cp_func")() == 1252);
    unlock();
    // Ask for a Windows locale name explicitly, independently of legacy CRT
    // language_country.codepage spelling used by the code-page checks above.
    const char *selected = current(LC_ALL, "en-US");
    auto names = API(m, ___lc_locale_name_func)();
    REQUIRE(names && names[LC_CTYPE]);
    if (!selected || _wcsicmp(names[LC_CTYPE], L"en-US")) {
        std::fprintf(stderr, "Unverified: LC_CTYPE name is %ls after requesting en-US\n", names[LC_CTYPE]);
        checked.erase({m.name, "___lc_locale_name_func"});
    }
    REQUIRE(current(LC_ALL, "C") && config(saved) == _ENABLE_PER_THREAD_LOCALE);
}

void file_modes() {
    Module io("api-ms-win-crt-stdio-l1-1-0.dll"), fs("api-ms-win-crt-filesystem-l1-1-0.dll");
    auto def = API(io, _set_fmode);
    REQUIRE(def(_O_BINARY) == 0);
    int *commit = API(io, __p__commode)();
    REQUIRE(commit);
    constexpr int commit_to_disk = 0x4000; // _IOCOMMIT in the CRT.
    int previous = *commit;
    *commit = commit_to_disk;
    REQUIRE(*API(io, __p__commode)() == commit_to_disk);
    *commit = previous;
    FILE *f = API(io, _wfopen)(L"runtime.txt", L"w+b");
    REQUIRE(f);
    API(fs, _lock_file)(f);
    int fd = API(io, _fileno)(f);
    REQUIRE(fd >= 0);
    auto mode = API(io, _setmode);
    REQUIRE(mode(fd, _O_TEXT) == _O_BINARY);
    REQUIRE(API(io, fputs)("A\nB", f) >= 0 && API(io, fflush)(f) == 0);
    REQUIRE(mode(fd, _O_BINARY) == _O_TEXT && API(io, fseek)(f, 0, SEEK_SET) == 0);
    char line[10];
    REQUIRE(API(io, fgets)(line, 10, f) == line && !std::strcmp(line, "A\r\n"));
    REQUIRE(API(io, fgets)(line, 10, f) == line && !std::strcmp(line, "B"));
    REQUIRE(!API(io, fgets)(line, 10, f) && API(io, feof)(f));
    API(fs, _unlock_file)(f);
    REQUIRE(API(io, fclose)(f) == 0);
    struct _stat64i32 st{};
    REQUIRE(API(fs, _stat64i32)("runtime.txt", &st) == 0 && st.st_size == 4 && (st.st_mode & _S_IFREG));
    REQUIRE(API(fs, remove)("runtime.txt") == 0 && GetFileAttributesW(L"runtime.txt") == INVALID_FILE_ATTRIBUTES);
}

void startup() {
    Module r("api-ms-win-crt-runtime-l1-1-0.dll"), e("api-ms-win-crt-environment-l1-1-0.dll");
    REQUIRE(API(r, _configure_narrow_argv)(_crt_argv_unexpanded_arguments) == 0);
    auto argv = *API(r, __p___argv)();
    REQUIRE(argv && !std::strcmp(argv[1], "coin-runtime") && !std::strcmp(argv[2], "17") && !argv[3]);
    std::string tail = API(r, _get_narrow_winmain_command_line)();
    tail.erase(tail.find_last_not_of(" \t") + 1);
    REQUIRE(tail == "coin-runtime 17");
    REQUIRE(API(r, _initialize_narrow_environment)() == 0 && API(e, getenv)("PATH"));
    REQUIRE(API(e, _putenv_s)("TUXBOX_CRT_ROUNDTRIP", "fixture") == 0 &&
            !std::strcmp(API(e, getenv)("TUXBOX_CRT_ROUNDTRIP"), "fixture"));
    char msg[80]{};
    REQUIRE(r.function<errno_t (*)(char *, size_t, int)>("strerror_s")(msg, sizeof(msg), ENOENT) == 0 &&
            !std::strcmp(msg, API(r, strerror)(ENOENT)));
    static int trace;
    _PVFV initializers[] = {[] { trace = 7; }, nullptr, [] { trace = trace * 10 + 3; }};
    API(r, _initterm)(initializers, initializers + 3);
    REQUIRE(trace == 73);
    _PIFV fallible[] = {[] { return 0; }, nullptr, [] { return 13; },
                        [] {
                            trace = 0;
                            return 0;
                        }};
    REQUIRE(API(r, _initterm_e)(fallible, fallible + 4) == 13 && trace == 73);
    auto set = API(r, _set_invalid_parameter_handler);
    auto old = set([](const wchar_t *, const wchar_t *, const wchar_t *, unsigned, uintptr_t) { ++trace; });
    API(r, _invalid_parameter_noinfo)();
    REQUIRE(trace == 74);
    set(old);
}

void native_memory() {
    Module m("vcruntime140.dll", LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
    char a[16]{};
    auto copy = m.function<void *(*)(void *, const void *, size_t)>("memcpy");
    auto move = m.function<void *(*)(void *, const void *, size_t)>("memmove");
    auto fill = m.function<void *(*)(void *, int, size_t)>("memset");
    REQUIRE(fill(a, '_', 12) == a && a[11] == '_' && copy(a, "aba", 3) == a);
    REQUIRE(move(a + 1, a, 3) == a + 1 && !std::memcmp(a, "aaba_", 5));
    REQUIRE(m.function<int (*)(const void *, const void *, size_t)>("memcmp")(a, "aabb", 4) < 0);
    REQUIRE(m.function<const void *(*)(const void *, int, size_t)>("memchr")(a, 'b', 4) == a + 2);
    a[4] = 0;
    const wchar_t w[] = L"aaba";
    using Find = const char *(*)(const char *, int);
    using WFind = const wchar_t *(*)(const wchar_t *, wchar_t);
    REQUIRE(m.function<Find>("strchr")(a, 'a') == a && m.function<Find>("strrchr")(a, 'a') == a + 3);
    REQUIRE(m.function<WFind>("wcschr")(w, L'a') == w && m.function<WFind>("wcsrchr")(w, L'a') == w + 3);
    REQUIRE(m.function<const char *(*)(const char *, const char *)>("strstr")(a, "ab") == a + 1);
    REQUIRE(m.function<const wchar_t *(*)(const wchar_t *, const wchar_t *)>("wcsstr")(w, L"ab") == w + 1);
}
void native_threads() {
    Module m("msvcp140.dll", LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
    using Status = _Thrd_result;
    constexpr auto ok = Status::_Success;
    auto lock = m.function<decltype(&_Mtx_lock)>("_Mtx_lock");
    auto unlock = m.function<decltype(&_Mtx_unlock)>("_Mtx_unlock");
    auto wait = m.function<decltype(&_Cnd_wait)>("_Cnd_wait");
    auto broadcast = m.function<decltype(&_Cnd_broadcast)>("_Cnd_broadcast");
    _Mtx_internal_imp_t mutex{};
    _Cnd_internal_imp_t cond{};
    m.function<void (*)(_Mtx_t, int)>("_Mtx_init_in_situ")(&mutex, _Mtx_plain);
    m.function<void (*)(_Cnd_t)>("_Cnd_init_in_situ")(&cond);
    struct Work {
        _Mtx_t mutex;
        _Cnd_t cond;
        decltype(lock) take;
        decltype(unlock) release;
        decltype(broadcast) notify;
        bool ready;
    } work{&mutex, &cond, lock, unlock, broadcast, false};
    REQUIRE(lock(&mutex) == ok);
    auto worker = CreateThread(
        nullptr, 0,
        [](void *p) -> DWORD {
            auto &w = *static_cast<Work *>(p);
            REQUIRE(w.take(w.mutex) == Status::_Success);
            w.ready = true;
            REQUIRE(w.notify(w.cond) == Status::_Success && w.release(w.mutex) == Status::_Success);
            return 0;
        },
        &work, 0, nullptr);
    REQUIRE(worker);
    while (!work.ready)
        REQUIRE(wait(&cond, &mutex) == ok);
    REQUIRE(unlock(&mutex) == ok && WaitForSingleObject(worker, 5000) == WAIT_OBJECT_0);
    CloseHandle(worker);
    m.function<void (*)(_Cnd_t)>("_Cnd_destroy_in_situ")(&cond);
    m.function<void (*)(_Mtx_t)>("_Mtx_destroy_in_situ")(&mutex);
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    REQUIRE(m.function<unsigned (*)()>("_Thrd_hardware_concurrency")() == info.dwNumberOfProcessors);
    REQUIRE(static_cast<DWORD>(m.function<long (*)()>(
                "?GetCurrentThreadId@platform@details@Concurrency@@YAJXZ")()) == GetCurrentThreadId());
    LARGE_INTEGER freq{}, begin{}, end{};
    REQUIRE(QueryPerformanceFrequency(&freq));
    REQUIRE(m.function<long long (*)()>("_Query_perf_frequency")() == freq.QuadPart);
    REQUIRE(QueryPerformanceCounter(&begin));
    m.function<void (*)()>("_Thrd_yield")();
    auto counter = m.function<long long (*)()>("_Query_perf_counter")();
    REQUIRE(QueryPerformanceCounter(&end) && begin.QuadPart <= counter && counter <= end.QuadPart);
    FILETIME ft{};
    GetSystemTimeAsFileTime(&ft);
    ULARGE_INTEGER epoch{};
    epoch.HighPart = ft.dwHighDateTime;
    epoch.LowPart = ft.dwLowDateTime;
    auto ticks = m.function<long long (*)()>("_Xtime_get_ticks")();
    REQUIRE(std::abs(ticks - static_cast<long long>(epoch.QuadPart - 116444736000000000ULL)) < 20000000);
}

void native_error_codes() {
    Module m("msvcp140.dll", LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
    auto syserror = m.function<const char *(*)(int)>("?_Syserror_map@std@@YAPEBDH@Z");
    auto winerror = m.function<int (*)(int)>("?_Winerror_map@std@@YAHH@Z");
    auto message = m.function<DWORD (*)(DWORD, char *, DWORD)>("?_Winerror_message@std@@YAKKPEADK@Z");
    REQUIRE(winerror(ERROR_FILE_NOT_FOUND) == ENOENT && winerror(ERROR_ACCESS_DENIED) == EACCES);
    REQUIRE(syserror(ENOENT) && !_stricmp(syserror(ENOENT), "no such file or directory"));
    char reference[512]{}, actual[512]{};
    REQUIRE(FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, nullptr,
                           ERROR_FILE_NOT_FOUND, 0, reference, sizeof(reference), nullptr));
    size_t length = std::strcspn(reference, "\r\n");
    REQUIRE(length > 0 && message(ERROR_FILE_NOT_FOUND, actual, sizeof(actual)) >= length &&
            !std::strncmp(reference, actual, length));
}

int wide_scan(const wchar_t *input, const wchar_t *format, ...) {
    Module io("api-ms-win-crt-stdio-l1-1-0.dll");
    va_list args;
    va_start(args, format);
    int result = API(io, __stdio_common_vswscanf)(0, input, std::wcslen(input), format, nullptr, args);
    va_end(args);
    return result;
}
void runtime_configuration() {
    Module r("api-ms-win-crt-runtime-l1-1-0.dll");
    EXCEPTION_RECORD record{};
    record.ExceptionCode = 0xe0000042;
    CONTEXT context{};
    RtlCaptureContext(&context);
    EXCEPTION_POINTERS pointers{&record, &context};
    REQUIRE(API(r, _seh_filter_exe)(record.ExceptionCode, &pointers) == EXCEPTION_CONTINUE_SEARCH);
    REQUIRE(API(r, _seh_filter_dll)(record.ExceptionCode, &pointers) == EXCEPTION_CONTINUE_SEARCH);
}
void math_callback() {
    Module math("api-ms-win-crt-math-l1-1-0.dll");
    static bool called;
    auto set = API(math, __setusermatherr);
    set([](_exception *e) -> int {
        called = e && e->type == _DOMAIN && e->arg1 == 2.;
        if (called)
            e->retval = 17.;
        return called;
    });
    auto acos_fn = math.function<double (*)(double)>("acos");
    double result = acos_fn(2.);
    set(nullptr);
    if (!called || result != 17.) {
        std::fprintf(stderr, "Unverified: acos domain callback called=%d, result=%g\n", called, result);
        checked.erase({math.name, "__setusermatherr"});
    }
}
void wide_scanning() {
    int a{}, b{};
    REQUIRE(wide_scan(L"-12-2a", L"%d-%x", &a, &b) == 2 && a == -12 && b == 42);
}

constexpr const char *crt_runtime = "api-ms-win-crt-runtime-l1-1-0.dll";
constexpr const char *crt_stdio = "api-ms-win-crt-stdio-l1-1-0.dll";
constexpr const char *vc_runtime = "vcruntime140.dll";
void marker(char c) {
    DWORD written{};
    REQUIRE(WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), &c, 1, &written, nullptr) && written == 1);
}
// Observe the GUI assertion path before the child's modal dialog becomes interactive.
LRESULT CALLBACK observe_assert_dialog(int code, WPARAM wparam, LPARAM lparam) {
    if (code == HCBT_CREATEWND || code == HCBT_ACTIVATE) {
        char class_name[64]{};
        GetClassNameA(reinterpret_cast<HWND>(wparam), class_name, sizeof(class_name));
        if (!std::strcmp(class_name, "#32770")) {
            marker('G');
            ExitProcess(36);
        }
    }
    return CallNextHookEx(nullptr, code, wparam, lparam);
}
void child_case(const std::string &name) {
    scenario = name.c_str();
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    Module r(crt_runtime);
    if (name == "_invalid_parameter_noinfo_noreturn") {
        API(r, _set_invalid_parameter_handler)(
            [](const wchar_t *, const wchar_t *, const wchar_t *, unsigned, uintptr_t) { marker('I'); });
        API(r, _invalid_parameter_noinfo_noreturn)();
    }
    if (name == "_invoke_watson")
        API(r, _invoke_watson)(nullptr, nullptr, nullptr, 0, 0);
    if (name == "_set_app_type") {
        API(r, _set_app_type)(_crt_gui_app);
        HHOOK hook = SetWindowsHookExW(WH_CBT, observe_assert_dialog, nullptr, GetCurrentThreadId());
        REQUIRE(hook);
        API(r, _wassert)(L"homebrew GUI diagnostic", L"coin.cpp", 8);
        REQUIRE(UnhookWindowsHookEx(hook));
        fail("GUI assertion returned without a dialog", __LINE__);
    }
    if (name == "puts") {
        Module io(crt_stdio);
        REQUIRE(API(io, puts)("coin") >= 0 && API(io, fflush)(nullptr) == 0);
        ExitProcess(30);
    }
    if (name == "_purecall") {
        Module vc(vc_runtime, LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
        API(vc, _set_purecall_handler)([] { ExitProcess(31); });
        vc.function<void (*)()>("_purecall")();
    }
    if (name == "__std_terminate" || name == "terminate") {
        auto host = GetModuleHandleA("ucrtbase.dll");
        auto set = reinterpret_cast<void (*)(void (*)())>(GetProcAddress(host, "set_terminate"));
        REQUIRE(set);
        set([] { ExitProcess(32); });
        if (name == "terminate")
            r.function<void (*)()>("terminate")();
        else {
            Module vc(vc_runtime, LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
            vc.function<void (*)()>("__std_terminate")();
        }
    }
    if (name == "raise" || name == "abort" || name == "_wassert") {
        auto sig = reinterpret_cast<decltype(&signal)>(GetProcAddress(GetModuleHandleA("ucrtbase.dll"), "signal"));
        REQUIRE(sig);
        REQUIRE(sig(SIGABRT, [](int n) {
                    if (n == SIGABRT)
                        ExitProcess(33);
                }) != SIG_ERR);
        if (name == "raise")
            API(r, raise)(SIGABRT);
        if (name == "abort")
            API(r, abort)();
        if (name == "_wassert") {
            API(r, _set_app_type)(_crt_console_app);
            API(r, _wassert)(L"coin fixture", L"coin.cpp", 7);
        }
    } else if (name == "_crt_at_quick_exit") {
        REQUIRE(API(r, _crt_at_quick_exit)([] { marker('Q'); }) == 0);
        auto quick =
            reinterpret_cast<void (*)(int)>(GetProcAddress(GetModuleHandleA("ucrtbase.dll"), "quick_exit"));
        REQUIRE(quick);
        quick(34);
    } else {
        REQUIRE(API(r, _crt_atexit)([] { marker('A'); }) == 0);
        REQUIRE(API(r, _crt_atexit)([] { marker('B'); }) == 0);
        if (name == "exit")
            API(r, exit)(35);
        if (name == "_exit")
            API(r, _exit)(35);
        if (name == "_Exit")
            API(r, _Exit)(35);
        if (name == "_cexit") {
            API(r, _cexit)();
            marker('R');
            ExitProcess(35);
        }
        if (name == "_c_exit") {
            API(r, _c_exit)();
            marker('R');
            ExitProcess(35);
        }
        if (name == "_register_thread_local_exe_atexit_callback") {
            API(r, _register_thread_local_exe_atexit_callback)([](void *, DWORD reason, void *) {
                if (reason == DLL_PROCESS_DETACH)
                    marker('T');
            });
            API(r, exit)(35);
        }
    }
    fail("child operation returned unexpectedly", __LINE__);
}

void exit_checks() {
    struct Case {
        const char *name;
        const char *dll;
        DWORD code;
        const char *output;
    };
    const Case cases[] = {
        {"_invalid_parameter_noinfo_noreturn", crt_runtime, 0xc0000417, "I"},
        {"_invoke_watson", crt_runtime, 0xc0000417, ""},
        {"_set_app_type", crt_runtime, 36, "G"},
        {"puts", crt_stdio, 30, "coin\r\n"},
        {"exit", crt_runtime, 35, "BA"},
        {"_exit", crt_runtime, 35, ""},
        {"_Exit", crt_runtime, 35, ""},
        {"_cexit", crt_runtime, 35, "BAR"},
        {"_c_exit", crt_runtime, 35, "R"},
        {"_crt_at_quick_exit", crt_runtime, 34, "Q"},
        {"_register_thread_local_exe_atexit_callback", crt_runtime, 35, "TBA"},
        {"raise", crt_runtime, 33, ""},
        {"abort", crt_runtime, 33, ""},
        {"_wassert", crt_runtime, 33, ""},
        {"terminate", crt_runtime, 32, ""},
        {"__std_terminate", vc_runtime, 32, ""},
        {"_purecall", vc_runtime, 31, ""},
    };
    for (auto c : cases) {
        if (verify_native_ucrt && std::strcmp(c.name, "_invoke_watson"))
            continue;
        SECURITY_ATTRIBUTES sa{sizeof(sa), nullptr, TRUE};
        HANDLE file = CreateFileA("exit-output.tmp", GENERIC_READ | GENERIC_WRITE, 0, &sa, CREATE_ALWAYS,
                                  FILE_ATTRIBUTE_NORMAL, nullptr);
        REQUIRE(file != INVALID_HANDLE_VALUE);
        STARTUPINFOA si{sizeof(si)};
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdOutput = file;
        bool console_assert = !std::strcmp(c.name, "_wassert");
        si.hStdError = console_assert ? file : GetStdHandle(STD_ERROR_HANDLE);
        si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        PROCESS_INFORMATION child{};
        std::string command = std::string(verify_native_ucrt ? "runtime_import_probe.exe --native-child "
                                                             : "runtime_import_probe.exe --child ") +
                              c.name;
        REQUIRE(CreateProcessA("runtime_import_probe.exe", command.data(), nullptr, nullptr, TRUE, 0, nullptr,
                               nullptr, &si, &child));
        if (WaitForSingleObject(child.hProcess, 5000) != WAIT_OBJECT_0) {
            TerminateProcess(child.hProcess, 1);
            fail(c.name, __LINE__);
        }
        DWORD status{}, count{};
        REQUIRE(GetExitCodeProcess(child.hProcess, &status));
        CloseHandle(child.hThread);
        CloseHandle(child.hProcess);
        REQUIRE(SetFilePointer(file, 0, nullptr, FILE_BEGIN) != INVALID_SET_FILE_POINTER);
        char output[512]{};
        REQUIRE(ReadFile(file, output, sizeof(output) - 1, &count, nullptr));
        CloseHandle(file);
        if (std::strcmp(c.name, "_invoke_watson") == 0 && status == 0x80000100 && count == 0) {
            std::fputs("Unverified: Proton reported an unimplemented _invoke_watson export\n", stderr);
            continue;
        }
        bool expected_output = console_assert
            ? std::strstr(output, "coin fixture") && std::strstr(output, "coin.cpp") && std::strchr(output, '7')
            : std::string(output, count) == c.output;
        if ((status != c.code && !(c.code == 0xc0000417 && status == 0xc0000409)) || !expected_output) {
            std::fprintf(stderr, "%s exited %lu, output '%s'\n", c.name, status, output);
            fail(c.name, __LINE__);
        }
        checked.emplace(Import{c.dll, c.name}, scenario);
    }
    if (!verify_native_ucrt) {
        checked.emplace(Import{crt_runtime, "_crt_atexit"}, scenario);
        checked.emplace(Import{vc_runtime, "_set_purecall_handler"}, scenario);
    }
    REQUIRE(DeleteFileA("exit-output.tmp"));
}

} // namespace

int main(int argc, char **args) {
    if (argc == 3 && !std::strcmp(args[1], "--native-child")) {
        verify_native_ucrt = true;
        child_case(args[2]);
        return 1;
    }
    if (argc == 2 && !std::strcmp(args[1], "--native-ucrt")) {
        REQUIRE(std::freopen("native-errors.txt", "w", stderr));
        verify_native_ucrt = true;
        scenario = "allocation callback receives requested size";
        allocation_callback();
        scenario = "English (United States) locale name";
        locale_data();
        scenario = "standard and daylight timezone names";
        calendar();
        scenario = "Watson terminates child with the CRT failure status";
        exit_checks();
        const Import required[] = {
            {crt_runtime, "_set_new_handler"},
            {"api-ms-win-crt-heap-l1-1-0.dll", "_callnewh"},
            {"api-ms-win-crt-locale-l1-1-0.dll", "___lc_locale_name_func"},
            {"api-ms-win-crt-time-l1-1-0.dll", "__tzname"},
            {crt_runtime, "_invoke_watson"},
        };
        FILE *report = std::fopen("native.tsv", "wb");
        REQUIRE(report);
        REQUIRE(std::fprintf(report, "dependency\tsymbol\tevidence\n") > 0);
        for (const auto &key : required) {
            REQUIRE(checked.count(key));
            REQUIRE(std::fprintf(report, "%s\t%s\t%s\n", key.first.c_str(), key.second.c_str(),
                                 checked.at(key).c_str()) > 0);
        }
        REQUIRE(!std::fclose(report));
        return 0;
    }
    if (argc == 3 && !std::strcmp(args[1], "--child")) {
        child_case(args[2]);
        return 1;
    }

    struct Test {
        const char *label;
        void (*run)();
    };
    const Test tests[] = {
        {"signed and unsigned 64-bit and floating-point parsing", conversion},
        {"floating-point quadrants, rounding, decomposition and classification", arithmetic},
        {"character classes, bounded comparisons, copies and tokenization", strings},
        {"sort, binary lookup, signed division and random sequences", sort_and_random},
        {"allocation, resize, zero fill and allocation mode", allocation},
        {"allocation handler receives requested byte count", allocation_callback},
        {"UTC and local dates, locale name tables and formatted calendar", calendar},
        {"per-thread locale selection and code pages", locale_data},
        {"locked CRT file, text translation, binary readback and EOF", file_modes},
        {"narrow startup arguments, environment, initializer order and error callback", startup},
        {"Scarlett mutex handoff, condition wakeup, thread identity and clocks", native_threads},
        {"Scarlett errno mapping and operating system error message", native_error_codes},
        {"child exit code, cleanup callback order and captured output", exit_checks},
        {"CRT filters leave an application exception for its caller", runtime_configuration},
        {"math domain error handler supplies the acos result", math_callback},
        {"wide formatted numeric scan", wide_scanning},
        {"Scarlett memory movement, overlapping copies and character search", native_memory},
    };
    for (auto test : tests) {
        scenario = test.label;
        test.run();
    }
    std::puts("dependency\tsymbol\tevidence");
    for (const auto &[k, e] : checked)
        std::printf("%s\t%s\t%s\n", k.first.c_str(), k.second.c_str(), e.c_str());
    return std::fflush(stdout) != 0 || std::ferror(stdout);
}
