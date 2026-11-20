// Compile this single translation unit both with and without the sample's GXDK tag.
// It records on a list supplied by another module, just as middleware does.
#include "graphics.h"

#ifdef FIXTURE_GXDK
#pragma section(".xbld", read)
extern "C" __declspec(allocate(".xbld")) const char sample_edition[] =
    "_xbld_edition_sdktype=GXDK," FIXTURE_GXDK;
#pragma comment(linker, "/include:sample_edition")
#endif

extern "C" __declspec(dllexport) void record_draw(ID3D12GraphicsCommandList* list,
    ID3D12CommandSignature* signature, ID3D12Resource* args, ID3D12Resource* counts) {
    using Indirect = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12CommandSignature*,
        UINT, ID3D12Resource*, UINT64, ID3D12Resource*, UINT64);
#ifdef FIXTURE_GXDK
    using Packet = void (WINAPI*)(ID3D12GraphicsCommandList*, BYTE, const void*, UINT);
    const auto packet = xbox_method<Packet>(list, 59);
    // PIX v2 begin, no arguments, ANSI label, with a color and trailing zero.
    const UINT64 begin[] = {0x00000000000f3083, 0xffff00ff, 0x74736574};
    packet(list, 1, begin, sizeof(begin));
#ifdef FIXTURE_PIX_TIMING
    using Timing = void (WINAPI*)(ID3D12GraphicsCommandList*, BYTE);
    auto timing = xbox_method<Timing>(list, 60);
    timing(list, 1);
    timing(list, 0);
#endif
    constexpr UINT slot = 61;
#else
    constexpr UINT slot = 59;
#endif
    auto draw = xbox_method<Indirect>(list, slot);
    draw(list, signature, 1, args, 16, counts, counts ? 4 : 0);
#ifdef FIXTURE_GXDK
    const UINT64 end = 1;
    packet(list, 0, &end, sizeof(end));
#endif
}
