// Build in an x64 MSVC developer shell with C++/CX. The native runtime turns a
// C++ allocation failure into the platform exception supplied by this callback.
#include <new>
#include <cstdio>
#include "import_probe.h"

using OomCallback = void *(__stdcall *)();
static IUnknown *preallocated;

static int callbacks;
static void *__stdcall provide_oom() {
    ++callbacks;
    preallocated->AddRef();
    return preallocated;
}

int main(Platform::Array<Platform::String ^> ^) {
    scenario = "Scarlett C++/CX out-of-memory callback";
    Module vc("vcruntime140.dll", LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
    require_app_local_module(GetModuleHandleA("vccorlib140.dll"), "vccorlib140.dll");
    auto set = vc.function<void (*)(OomCallback)>("_SetWinRTOutOfMemoryExceptionCallback");
    auto oom = ref new Platform::OutOfMemoryException();
    preallocated = reinterpret_cast<IUnknown *>(oom);
    set(provide_oom);
    bool caught = false;
    try {
        std::_Xbad_alloc();
    } catch (Platform::OutOfMemoryException ^ e) {
        caught = e == oom && e->HResult == E_OUTOFMEMORY;
    } catch (const std::bad_alloc &) {
        std::fputs("bad_alloc reached the C++ catch clause\n", stderr);
    }
    set(nullptr);
    REQUIRE(caught && callbacks == 1);
    FILE *out = nullptr;
    if (fopen_s(&out, "winrt.tsv", "wb") || !out)
        return 1;
    int count = std::fprintf(
        out, "dependency\tsymbol\tevidence\nvcruntime140.dll\t_SetWinRTOutOfMemoryExceptionCallback\tScarlett maps "
             "bad_alloc to the registered WinRT out-of-memory exception\n");
    return fclose(out) != 0 || count < 0;
}
