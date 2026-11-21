// Build with the DLL CRT so MSVC C++ ABI calls use the supplied Scarlett libraries.
#define _SILENCE_CXX17_UNCAUGHT_EXCEPTION_DEPRECATION_WARNING
#define _CRT_SECURE_NO_WARNINGS
#include <fstream>
#include <windows.h>
#include <cstdio>
#include <cstring>
#include <iomanip>
#include <iostream>
#include <locale>
#include <sstream>
#include <functional>
#include <system_error>
#include <share.h>
#include <eh.h>
struct _s_ThrowInfo;
using _ThrowInfo = const _s_ThrowInfo;
#include <ehdata_forceinclude.h>
#include <mutex>
#include <setjmp.h>
#include <xthreads.h>
extern "C" void runtime_hit(const char *, const char *, const void *) noexcept;

extern "C" void runtime_scenario(const char *) noexcept;
extern "C" int runtime_report() noexcept;

#define CHECK(expr)                                                                                                \
    do {                                                                                                           \
        if (!(expr)) {                                                                                             \
            std::fprintf(stderr, "line %d: %s\n", __LINE__, #expr);                                                \
            std::exit(1);                                                                                          \
        }                                                                                                          \
    } while (false)

template <class Char> struct Buffer : std::basic_streambuf<Char> {
    using Base = std::basic_streambuf<Char>;
    using Traits = typename Base::traits_type;
    void exercise() {
        Char in[]{'1', '2', '3'}, out[4]{}, tmp[3]{};
        this->_Init();
        Char *gf = in, *gn = in, *pf = out, *pn = out;
        int gc = 3, pc = 4;
        this->_Init(&gf, &gn, &gc, &pf, &pn, &pc);
        this->gbump(1);
        CHECK(gn == in + 1 && gc == 2);
        this->pbump(1);
        CHECK(pn == out + 1 && pc == 3);
        this->_Init();
        this->setg(in, in, in + 3);
        this->setp(out, out + 4);
        CHECK(this->eback() == in && this->egptr() == in + 3 && this->gptr() == in);
        CHECK(this->pbase() == out && this->epptr() == out + 4 && this->pptr() == out);
        CHECK(this->_Gnavail() == 3 && this->_Gninc() == in && this->_Gndec() == in);
        CHECK(this->_Pnavail() == 4 && this->_Pninc() == out);
        this->setp(out, out + 1, out + 4);
        CHECK(this->sputc('a') == 'a' && this->sputn(in, 2) == 2 && out[1] == 'a' && out[3] == '2');
        CHECK(this->sbumpc() == '1' && this->Base::xsgetn(tmp, 2) == 2 && tmp[0] == '2' && tmp[1] == '3');
        this->setp(out, out + 4);
        CHECK(this->Base::xsputn(in, 3) == 3 && out[0] == '1' && out[2] == '3');
        this->Base::_Lock();
        CHECK(this->Base::setbuf(out, 4) == this && this->Base::sync() == 0 && this->Base::showmanyc() == 0);
        this->Base::_Unlock();
        CHECK(this->Base::uflow() == Traits::eof());
        this->Base::imbue(std::locale::classic());
        CHECK(this->getloc() == std::locale::classic());
    }
};

template <class Char> void streams() {
    Buffer<Char> buffer;
    buffer.exercise();
    std::basic_stringbuf<Char> storage;
    std::basic_iostream<Char> s(&storage);
    CHECK(s.rdbuf() == &storage && s.tie() == nullptr && s.fill() == Char(' ') && s.good() && !s.bad());
    CHECK(s.width(4) == 0 && s.width() == 4);
    s.width(0);
    s.setf(std::ios_base::hex, std::ios_base::basefield);
    CHECK((s.flags() & std::ios_base::basefield) == std::ios_base::hex);
    s << std::dec << int(-17) << Char(' ') << unsigned(23) << Char(' ') << 31ULL;
    s.put(Char(' '));
    s << static_cast<unsigned short>(9) << Char(' ') << 11L;
    const Char word[] = {'o', 'k'};
    s.write(word, 2);
    s << std::flush<Char, std::char_traits<Char>>;
    CHECK(s.tellp() == std::streampos(16) && s.good());
    s.seekg(std::streampos(0));
    int parsed = 0;
    CHECK(s._Ipfx(false));
    s >> parsed;
    CHECK(parsed == -17 && s.tellg() == std::streampos(3));
    Char read[3]{};
    s.read(read, 3);
    CHECK(read[0] == ' ' && read[1] == '2' && read[2] == '3');
    CHECK(s.widen('X') == 'X' && s.imbue(std::locale::classic()) == s.getloc());
    s._Osfx();
    s.setstate(std::ios_base::badbit);
    CHECK(s.fail() && s.bad());
    s.clear();
    CHECK(s.good());
    std::basic_ostream<Char> output(&storage);
    std::basic_istream<Char> input(&storage);
    CHECK(output.good() && input.good());
}

void facets() {
    const auto &loc = std::locale::classic();
    const auto &c = std::use_facet<std::ctype<char>>(loc);
    const auto &w = std::use_facet<std::ctype<wchar_t>>(loc);
    CHECK(c.tolower('X') == 'x' && c.widen('x') == 'x' && w.widen('A') == L'A');
    const char text[] = "aBc";
    char destination[4]{};
    CHECK(c.widen(text, text + 3, destination) == text + 3 && !std::strcmp(destination, text));
    const auto &codec = std::use_facet<std::codecvt<char, char, std::mbstate_t>>(loc);
    std::mbstate_t state{};
    const char *from = nullptr;
    char *to = nullptr;
    CHECK(codec.always_noconv());
    CHECK(codec.in(state, text, text + 3, from, destination, destination + 3, to) == std::codecvt_base::noconv);
    CHECK(codec.out(state, text, text + 3, from, destination, destination + 3, to) == std::codecvt_base::noconv);
    CHECK(codec.unshift(state, destination, destination + 3, to) == std::codecvt_base::noconv);
    CHECK(std::use_facet<std::numpunct<char>>(loc).decimal_point() == '.');
    CHECK(std::use_facet<std::numpunct<wchar_t>>(loc).decimal_point() == L'.');
}

void exceptions() {
    struct Count {
        int &count;
        ~Count() {
            count = std::uncaught_exceptions();
            CHECK(std::uncaught_exception());
        }
    };
    CHECK(std::uncaught_exceptions() == 0 && !std::uncaught_exception());
    int n = 0;
    try {
        Count counter{n};
        std::_Xout_of_range("coin");
    } catch (const std::out_of_range &e) {
        CHECK(n == 1 && !std::strcmp(e.what(), "coin"));
        std::exception_ptr first = std::current_exception(), second = first;
        try {
            std::rethrow_exception(second);
        } catch (const std::out_of_range &copy) {
            CHECK(!std::strcmp(copy.what(), "coin"));
        }
    }
    bool caught = false;
    try {
        std::_Xlength_error("length");
    } catch (const std::length_error &e) {
        caught = !std::strcmp(e.what(), "length");
    }
    CHECK(caught);
    caught = false;
    try {
        std::_Xbad_alloc();
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    CHECK(caught);
    caught = false;
    try {
        std::function<void()> empty;
        empty();
    } catch (const std::bad_function_call &) {
        caught = true;
    }
    CHECK(caught);
}

void identities() {
    using namespace std;
    struct SampleFacet : locale::facet {};
    SampleFacet f;
    f.locale::facet::_Incref();
    f.locale::facet::_Incref();
    CHECK(f.locale::facet::_Decref() == nullptr);
    CHECK(f.locale::facet::_Decref() == &f);
    std::_Locinfo info("C");
    CHECK(std::wcsstr(reinterpret_cast<const wchar_t *>(info._W_Getdays()), L"Friday"));
    CHECK(std::wcsstr(reinterpret_cast<const wchar_t *>(info._W_Getmonths()), L"January"));
    struct Base : std::ios_base {
        Base() {
            _Init();
        }
    } base;
    CHECK(base.good() && base.precision() == 6);
    base.clear(std::ios_base::failbit, false);
    CHECK(base.fail());
    struct NarrowIos : std::basic_ios<char> {
        NarrowIos() {
            init();
        }
    } narrow;
    struct WideIos : std::basic_ios<wchar_t> {
        WideIos() {
            init();
        }
    } wide;
    CHECK(narrow.bad() && wide.bad());
    std::ostringstream format;
    format << std::setprecision(3) << 1.234567 << " ";
    int x{};
    format << static_cast<const void *>(&x);
    CHECK(format.str().find("1.23 ") == 0 && format.str().size() > 6);
    const auto &loc = std::locale::classic();
    using Codec = std::codecvt<char, char, std::mbstate_t>;
    struct FacetId {
        const char *symbol;
        std::locale::id *id;
    };
    FacetId ids[] = {
        {"?id@?$codecvt@DDU_Mbstatet@@@std@@2V0locale@2@A", &Codec::id},
        {"?id@?$ctype@D@std@@2V0locale@2@A", &std::ctype<char>::id},
        {"?id@?$ctype@_W@std@@2V0locale@2@A", &std::ctype<wchar_t>::id},
        {"?id@?$numpunct@D@std@@2V0locale@2@A", &std::numpunct<char>::id},
        {"?id@?$numpunct@_W@std@@2V0locale@2@A", &std::numpunct<wchar_t>::id},
    };
    for (size_t i = 0; i < 5; i++) {
        auto index = ids[i].id->_Get_index();
        CHECK(index > 0);
        for (size_t j = 0; j < i; j++)
            CHECK(ids[j].id->_Get_index() != index);
        runtime_hit("msvcp140.dll", ids[i].symbol, ids[i].id);
    }
    CHECK(std::cerr.good() && (std::cerr.flags() & std::ios_base::unitbuf));
    runtime_hit("msvcp140.dll", "?cerr@std@@3V?$basic_ostream@DU?$char_traits@D@std@@@1@A", &std::cerr);
    CHECK(std::has_facet<Codec>(loc));
}

struct Animal {
    virtual ~Animal() = default;
};
struct Cat : Animal {};
struct Dog : Animal {};
__declspec(noinline) void inspect_type(Animal *a) {
    CHECK(dynamic_cast<Cat *>(a) != nullptr && dynamic_cast<Dog *>(a) == nullptr);
    const auto &type = typeid(*a);
    CHECK(type == typeid(Cat) && type != typeid(Dog) && std::strstr(type.name(), "Cat"));
}

void runtime_types() {
    Cat pet;
    inspect_type(&pet);
    char name[] = "coin";
    std::exception original(name);
    name[0] = 'X';
    std::exception copy(original);
    CHECK(!std::strcmp(copy.what(), "coin"));
    bool caught = false;
    try {
        throw std::runtime_error("runtime");
    } catch (const std::runtime_error &e) {
        caught = !std::strcmp(e.what(), "runtime");
    }
    CHECK(caught);
    caught = false;
    try {
        std::_Throw_Cpp_error(std::_INVALID_ARGUMENT);
    } catch (const std::system_error &e) {
        caught = e.code() == std::errc::invalid_argument;
    }
    CHECK(caught);
    FILE *stream = std::_Fiopen("cpp-file-test.tmp", std::ios_base::out | std::ios_base::trunc, _SH_DENYNO);
    CHECK(stream && std::fputs("coin", stream) >= 0 && !std::fclose(stream));
    stream = std::fopen("cpp-file-test.tmp", "rb");
    char buf[5]{};
    CHECK(stream && std::fread(buf, 1, 4, stream) == 4 && !std::strcmp(buf, "coin"));
    CHECK(!std::fclose(stream) && !std::remove("cpp-file-test.tmp"));
}

extern "C" __declspec(dllimport) int *__processing_throw();
extern "C" __declspec(dllimport) EXCEPTION_RECORD **__current_exception();
extern "C" __declspec(dllimport) CONTEXT **__current_exception_context();

void exception_state() {
    struct Watch {
        bool &unwound;
        ~Watch() {
            CHECK(*__processing_throw() > 0 && __uncaught_exception() && __uncaught_exceptions() == 1);
            unwound = true;
        }
    };
    CHECK(!*__current_exception() && !*__current_exception_context());
    CHECK(!*__processing_throw() && !__uncaught_exception() && __uncaught_exceptions() == 0);
    bool unwound = false;
    try {
        Watch w{unwound};
        throw 41;
    } catch (int n) {
        CHECK(n == 41 && unwound && *__current_exception() &&
              (*__current_exception())->ExceptionCode == 0xe06d7363);
        CHECK(*__current_exception_context() && (*__current_exception_context())->Rip);
    }
    CHECK(!*__current_exception() && !*__current_exception_context());
}

template <class Stream> struct OwnedStream : Stream {
    explicit OwnedStream(std::basic_streambuf<typename Stream::char_type> *b) : Stream(b) {
    }
};
template <class Char> void stream_lifetime() {
    using Buf = std::basic_stringbuf<Char>;
    Buf storage;
    static int erased;
    auto check = [&](auto *s) {
        s->register_callback(
            [](std::ios_base::event e, std::ios_base &, int id) {
                if (e == std::ios_base::erase_event)
                    erased += id;
            },
            1);
    };
    {
        OwnedStream<std::basic_iostream<Char>> both(&storage);
        OwnedStream<std::basic_istream<Char>> in(&storage);
        OwnedStream<std::basic_ostream<Char>> out(&storage);
        check(&both);
        check(&in);
        check(&out);
    }
    CHECK(erased == 3);
}

void filter_check() {
    constexpr DWORD code = 0xE0000042; // Private, continuable exception raised by this fixture.
    int handled = 0;
    __try {
        RaiseException(code, 0, 0, nullptr);
    } __except (GetExceptionCode() == code ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH) {
        handled = 1;
    }
    CHECK(handled == 1);
}

// The Windows x64 unwind table names the handler used by this exact catching frame.
void record_handler(const void *function, const char *dll, const char *name) {
    DWORD64 base = 0;
    auto entry = RtlLookupFunctionEntry(reinterpret_cast<DWORD64>(function), &base, nullptr);
    CHECK(entry);
    auto info = reinterpret_cast<const unsigned char *>(base + entry->UnwindData);
    CHECK((info[0] & 7) == 1 && ((info[0] >> 3) & UNW_FLAG_EHANDLER));
    const auto aligned_codes = (info[2] + 1U) & ~1U;
    DWORD handler_rva;
    std::memcpy(&handler_rva, info + 4 + 2 * aligned_codes, sizeof(handler_rva));
    auto thunk = reinterpret_cast<const unsigned char *>(base + handler_rva);
    // The PE linker emits a RIP-relative indirect jump to the imported handler.
    CHECK(thunk[0] == 0xff && thunk[1] == 0x25);
    int displacement;
    std::memcpy(&displacement, thunk + 2, sizeof(displacement));
    const void *handler;
    std::memcpy(&handler, thunk + 6 + displacement, sizeof(handler));
    runtime_hit(dll, name, handler);
}

void retail_stream_catch() {
    struct RejectWrite : std::streambuf {
        bool called = false;
        int_type overflow(int_type) override {
            called = true;
            throw 17;
        }
    } buffer;
    std::ostream stream(&buffer);
    stream.put('x');
    CHECK(buffer.called && stream.bad());
    auto put = GetProcAddress(GetModuleHandleA("msvcp140.dll"),
                              "?put@?$basic_ostream@DU?$char_traits@D@std@@@std@@QEAAAEAV12@D@Z");
    CHECK(put);
    record_handler(reinterpret_cast<const void *>(put), "vcruntime140_1.dll", "__CxxFrameHandler4");
}

// Earlier MSVC headers imported these ABI entry points. Current headers inline
// or rename their callers. The fixture links the names used in the inventory.
namespace std {
class __declspec(dllimport) _Init_locks {
  public:
    _Init_locks() noexcept;
    ~_Init_locks() noexcept;
};
__declspec(dllimport) int _Execute_once(once_flag &, int (*)(void *, void *, void **), void *);
[[noreturn]] __declspec(dllimport) void _Throw_C_error(int);
} // namespace std

void legacy_stl() {
    std::_Init_locks lifetime;
    std::once_flag flag;
    int calls = 0;
    auto initialize = [](void *, void *state, void **) -> int {
        ++*static_cast<int *>(state);
        return 1;
    };
    CHECK(std::_Execute_once(flag, initialize, &calls) && std::_Execute_once(flag, initialize, &calls) &&
          calls == 1);
    bool caught = false;
    try {
        std::_Throw_C_error(static_cast<int>(_Thrd_result::_Busy));
    } catch (const std::system_error &e) {
        caught = e.code() == std::errc::device_or_resource_busy;
    }
    CHECK(caught);
    // Allocate two previously unused facet IDs through the original conversion export.
    auto dll = GetModuleHandleA("msvcp140.dll");
    auto convert =
        reinterpret_cast<size_t (*)(std::locale::id *)>(GetProcAddress(dll, "??Bid@locale@std@@QEAA_KXZ"));
    auto count = reinterpret_cast<const int *>(GetProcAddress(dll, "?_Id_cnt@id@locale@std@@0HA"));
    CHECK(convert && count);
    std::locale::id first, second;
    auto before = *count;
    auto a = convert(&first), b = convert(&second);
    CHECK(a == size_t(before + 1) && b == a + 1 && convert(&first) == a && *count == before + 2);
    runtime_hit("msvcp140.dll", "??Bid@locale@std@@QEAA_KXZ", reinterpret_cast<const void *>(convert));
    runtime_hit("msvcp140.dll", "?_Id_cnt@id@locale@std@@0HA", count);
    auto init = reinterpret_cast<void *(*)(bool)>(GetProcAddress(dll, "?_Init@locale@std@@CAPEAV_Locimp@12@_N@Z"));
    auto global =
        reinterpret_cast<void *(*)()>(GetProcAddress(dll, "?_Getgloballocale@locale@std@@CAPEAV_Locimp@12@XZ"));
    CHECK(init && global);
    auto original = global();
    CHECK(original && init(false) == original);
    auto old = std::locale::global(std::locale(std::locale::classic(), new std::numpunct<char>));
    auto changed = global();
    CHECK(changed && changed != original && init(false) == changed);
    std::locale::global(old);
    CHECK(global() == original);
    runtime_hit("msvcp140.dll", "?_Init@locale@std@@CAPEAV_Locimp@12@_N@Z", reinterpret_cast<const void *>(init));
    runtime_hit("msvcp140.dll", "?_Getgloballocale@locale@std@@CAPEAV_Locimp@12@XZ",
                reinterpret_cast<const void *>(global));
    struct Registered : std::ios_base {
        Registered() {
            _Init();
        }
    } stream;
    CHECK(stream._Stdstr == 0);
    std::ios_base::_Addstd(&stream);
    CHECK(stream._Stdstr > 0 && stream.good());
}

extern "C" __declspec(dllimport) __attribute__((returns_twice)) int __intrinsic_setjmp(jmp_buf);
void jump_roundtrip() {
    jmp_buf state;
    volatile int before_jump = 0;
    int rc = __intrinsic_setjmp(state);
    if (rc == 0) {
        before_jump = 7;
        longjmp(state, 23);
    }
    CHECK(rc == 23 && before_jump == 7);
}

extern "C" __declspec(dllimport) void *__GetPlatformExceptionInfo(int *);
void platform_exception() {
    int bad_alloc = 42;
    CHECK(__GetPlatformExceptionInfo(&bad_alloc) == nullptr && bad_alloc == 0);
    try {
        throw std::bad_alloc();
    } catch (const std::bad_alloc &) {
        CHECK(__GetPlatformExceptionInfo(&bad_alloc) == nullptr && bad_alloc != 0);
    }
}

void rtti_storage() {
    struct Left {
        virtual ~Left() = default;
        int left;
    };
    struct Right {
        virtual ~Right() = default;
        int right;
    };
    struct Pair : Left, Right {
    } pair;
    Right *base = &pair;
    auto delta = reinterpret_cast<char *>(base) - reinterpret_cast<char *>(&pair);
    CHECK(delta > 0);
    PMD offset{static_cast<int>(delta), -1, 0};
    auto dll = GetModuleHandleA("vcruntime140.dll");
    auto adjust = reinterpret_cast<void *(*)(void *, const PMD *)>(GetProcAddress(dll, "__AdjustPointer"));
    CHECK(adjust && adjust(&pair, &offset) == base);
    runtime_hit("vcruntime140.dll", "__AdjustPointer", reinterpret_cast<const void *>(adjust));

    // Keep the demangled names in a private list, like a DLL that is about to unload.
    struct TypeName {
        const char *cache;
        char decorated[32];
    };
    TypeName first{nullptr, ".?AVCoin@@"}, second{nullptr, ".?AVToken@@"};
    SLIST_HEADER names;
    InitializeSListHead(&names);
    auto name =
        reinterpret_cast<const char *(*)(TypeName *, SLIST_HEADER *)>(GetProcAddress(dll, "__std_type_info_name"));
    auto destroy = reinterpret_cast<void (*)(SLIST_HEADER *)>(GetProcAddress(dll, "__std_type_info_destroy_list"));
    CHECK(name && destroy && !std::strcmp(name(&first, &names), "class Coin") &&
          !std::strcmp(name(&second, &names), "class Token"));
    CHECK(QueryDepthSList(&names) == 2);
    destroy(&names);
    CHECK(QueryDepthSList(&names) == 0);
    runtime_hit("vcruntime140.dll", "__std_type_info_destroy_list", reinterpret_cast<const void *>(destroy));
}

int main() {
    runtime_scenario("Scarlett narrow stream buffer, formatted output and input positions");
    streams<char>();
    runtime_scenario("Scarlett wide stream buffer, formatted output and input positions");
    streams<wchar_t>();
    runtime_scenario("Scarlett classic locale, character facets and codecvt");
    facets();
    runtime_scenario("Scarlett exception types, unwinding and exception pointer rethrow");
    exceptions();
    runtime_scenario("Scarlett facet reference counts, IDs and stream formatting state");
    identities();
    runtime_scenario("Scarlett runtime type identity, exception ownership and native file stream");
    runtime_types();
    runtime_scenario("Scarlett exception records and counters during unwind and catch");
    exception_state();
    runtime_scenario("Scarlett derived stream destruction with erase callbacks");
    stream_lifetime<char>();
    stream_lifetime<wchar_t>();
    runtime_scenario("structured exception filter selects the homebrew exception");
    filter_check();
    record_handler(reinterpret_cast<const void *>(&filter_check), "vcruntime140.dll", "__C_specific_handler");
    runtime_scenario("Scarlett compiler unwind handler caught a typed homebrew exception");
    record_handler(reinterpret_cast<const void *>(&exception_state), "vcruntime140.dll", "__CxxFrameHandler3");
    runtime_scenario("Scarlett ostream caught a throwing stream buffer and set badbit");
    retail_stream_catch();
    runtime_scenario("Scarlett legacy locale, synchronization and registered stream lifetime");
    legacy_stl();
    runtime_scenario("Scarlett nonlocal jump restores the saved homebrew frame");
    jump_roundtrip();
    runtime_scenario("Scarlett distinguishes a caught allocation failure from no platform exception");
    platform_exception();
    runtime_scenario("Scarlett adjusts a multiple-inheritance base and releases cached type names");
    rtti_storage();
    return runtime_report();
}
