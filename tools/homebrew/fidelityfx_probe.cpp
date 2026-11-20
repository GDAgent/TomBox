// Public FidelityFX C ABI from the Xbox-GDK-Samples FidelityFX kit.
// Keep the probe independent of the console SDK headers.
#include "graphics.h"
#include <cstdint>

struct FfxHeader { uint64_t type; FfxHeader* next; };
struct ResolutionQuery {
    FfxHeader header;
    uint32_t display_width, display_height, quality;
    uint32_t* render_width;
    uint32_t* render_height;
};
using Query = uint32_t (*)(void**, FfxHeader*);

static int check_resolutions(Query query) {
    struct Case { uint32_t quality, width, height; };
    const Case modes[] = {{0, 1920, 1080}, {1, 1280, 720}, {3, 960, 540}, {4, 640, 360}};
    for (const auto& mode : modes) {
        uint32_t width = 0, height = 0;
        ResolutionQuery resolution{{0x10003, nullptr}, 1920, 1080, mode.quality, &width, &height};
        if (query(nullptr, &resolution.header) || width != mode.width || height != mode.height) {
            std::fprintf(stderr, "Quality %u returned %ux%u for 1920x1080\n", mode.quality, width, height);
            return 1;
        }
        std::printf("Quality %u: 1920x1080 -> %ux%u\n", mode.quality, width, height);
    }
    return 0;
}

static int create_fsr3(HMODULE loader) {
    struct Override { FfxHeader header; uint64_t id; };
    struct Version { FfxHeader header; uint32_t version; };
    struct Backend { FfxHeader header; ID3D12Device* device; };
    struct Upscale {
        FfxHeader header;
        uint32_t flags, render_width, render_height, display_width, display_height;
        void (*message)(uint32_t type, const wchar_t* text);
    };
    using Create = uint32_t (*)(void**, FfxHeader*, const void*);
    using Destroy = uint32_t (*)(void**, const void*);
    auto create = reinterpret_cast<Create>(GetProcAddress(loader, "ffxCreateContext"));
    auto destroy = reinterpret_cast<Destroy>(GetProcAddress(loader, "ffxDestroyContext"));
    if (!create || !destroy) return 1;
    ComPtr<ID3D12Device> device;
    CHECK(D3D12XboxCreateDevice(nullptr, nullptr, IID_PPV_ARGS(&device)));
    // Version ID returned by the pinned loader for its 3.1.6 provider.
    Override provider{{5, nullptr}, 0xf5a5ca1e00c01006ull};
    Version version{{0x1000b, &provider.header}, (4u << 22) | (1u << 12) | 2u};
    Backend backend{{0x02000002, &version.header}, device.Get()};
    Upscale upscale{{0x10000, &backend.header}, 0, 32, 32, 64, 64, nullptr};
    void* context = nullptr;
    auto result = create(&context, &upscale.header, nullptr);
    if (result || !context) {
        std::fprintf(stderr, "ffxCreateContext for FSR 3.1.6 returned %u, context %p\n", result, context);
        return 1;
    }
    return destroy(&context, nullptr) != 0;
}

int main(int argc, char** argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    const bool context = argc == 2 && !std::strcmp(argv[1], "--context");
    if (argc != 1 && !context) return 2;
    HMODULE loader = LoadLibraryW(L"amd_fidelityfx_loader_xbox.dll");
    if (!loader) {
        std::fprintf(stderr, "LoadLibrary(FidelityFX) failed with %lu\n", GetLastError());
        return 1;
    }
    auto query = reinterpret_cast<Query>(GetProcAddress(loader, "ffxQuery"));
    if (!query) return 1;
    int result = check_resolutions(query);
    if (result == 0 && context) result = create_fsr3(loader);
    FreeLibrary(loader);
    return result;
}
