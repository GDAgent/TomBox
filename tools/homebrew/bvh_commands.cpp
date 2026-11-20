// Reproduce the slot and argument layout observed in the 250405 BVH builder.
#include "graphics.h"

#pragma section(".xbld", read)
extern "C" __declspec(allocate(".xbld")) const char bvh_edition[] = "_xbld_edition_sdktype=GXDK,250405";
#pragma comment(linker, "/include:bvh_edition")

extern "C" __declspec(dllexport) void order_compute_writes(ID3D12GraphicsCommandList* list) {
    D3D12_GLOBAL_BARRIER barrier{D3D12_BARRIER_SYNC_COMPUTE_SHADING, D3D12_BARRIER_SYNC_COMPUTE_SHADING,
                                 D3D12_BARRIER_ACCESS_UNORDERED_ACCESS, D3D12_BARRIER_ACCESS_UNORDERED_ACCESS};
    D3D12_BARRIER_GROUP group{D3D12_BARRIER_TYPE_GLOBAL, 1};
    group.pGlobalBarriers = &barrier;
    using Barrier = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_BARRIER_GROUP*);
    xbox_method<Barrier>(list, 146)(list, 1, &group);
}

extern "C" __declspec(dllexport) void dispatch_threads(ID3D12GraphicsCommandList* list, UINT x, UINT flags) {
    using Dispatch = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, UINT, UINT);
    xbox_method<Dispatch>(list, 80)(list, x, 1, 1, flags);
}

extern "C" __declspec(dllexport) void clear_counter(ID3D12GraphicsCommandList* list, UINT64 dest, UINT flags) {
    using Write = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, UINT, UINT);
    xbox_method<Write>(list, 90)(list, dest, 0, flags);
}
