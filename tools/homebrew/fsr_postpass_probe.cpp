// Dispatch the retail FSR4 border-clear passes through TuxBox. The library is
// read as data; only the verified DXBC containers are passed to the device.
#define NOMINMAX
#include "graphics.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <vector>

namespace {
struct Pass {
    uint64_t hash;
    UINT origin, pitch, planes, plane_stride;
    D3D12_SHADER_BYTECODE code{};
};

// Byte offsets of the tensor planes and their row pitches in the supplied model.
const std::array<Pass, 13> layout = {{
    {0x260286bb30cdb3a2ULL, 0, 0x3c20, 1, 0x0},
    {0x9df42fe0b51b9d84ULL, 0x7f4bc0, 0x3c20, 1, 0x0},
    {0x8a311d1c2aeafcbcULL, 0, 0x3c20, 1, 0x0},
    {0x4d52d5840de5e864ULL, 0x1001100, 0x1e20, 2, 0x200200},
    {0xd956f5bb196bf80fULL, 0xbf4fc0, 0x1e20, 2, 0x200200},
    {0xed684d5ae2b2fe5eULL, 0x7f4bc0, 0x1e20, 2, 0x200200},
    {0xe0aa9e687a00a5e9ULL, 0x1001100, 0xf20, 4, 0x81820},
    {0xfef322f6e722ba45ULL, 0x1207180, 0xf20, 4, 0x81820},
    {0x16e52abe87026027ULL, 0x1001100, 0xf20, 4, 0x81820},
    {0x85ad5cd1b33aea44ULL, 0x7f4bc0, 0x1e20, 2, 0x200200},
    {0x094ef6ec5f38debcULL, 0xbf4fc0, 0x1e20, 2, 0x200200},
    {0x626516c3a62ae261ULL, 0, 0x3c20, 1, 0x0},
    {0x854150f7ec86eaf9ULL, 0x7f4bc0, 0x3c20, 1, 0x0},
}};

int dispatch(const Pass& pass, UINT index, UINT width, UINT height, UINT max_height) {
    constexpr UINT guard = 0xcafe1234;
    const UINT columns = std::min(pass.pitch / 16, 1 + width + 5);
    const UINT rows = std::min(max_height + 1, 1 + height + 1);
    const UINT bytes = pass.origin + (pass.planes - 1) * pass.plane_stride + rows * pass.pitch + 256;
    Frame frame;
    if (frame.init(1, 1, false, bytes)) return 1;
    auto* dev = frame.dev.Get();
    auto* list = frame.list.Get();
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[0].Descriptor.ShaderRegister = 11;
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[1].Constants = {0, 0, 56};
    D3D12_ROOT_SIGNATURE_DESC desc{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    CHECK(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    ComPtr<ID3D12RootSignature> root;
    CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)));
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                     UINT, const void*, REFIID, void**);
    D3D12_COMPUTE_PIPELINE_STATE_DESC cs{};
    cs.pRootSignature = root.Get();
    cs.CS = pass.code;
    ComPtr<ID3D12PipelineState> pso;
    CHECK(xbox_method<Create>(dev, 62)(dev, &cs, 0, nullptr, IID_PPV_ARGS(&pso)));

    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    auto buffer = frame.readback->GetDesc();
    ComPtr<ID3D12Resource> upload, output;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* mapped;
    CHECK(upload->Map(0, nullptr, &mapped));
    std::fill_n(static_cast<UINT*>(mapped), bytes / 4, guard);
    upload->Unmap(0, nullptr);
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    buffer.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_COPY_DEST,
                                       nullptr, IID_PPV_ARGS(&output)));
    list->CopyBufferRegion(output.Get(), 0, upload.Get(), 0, bytes);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
    list->ResourceBarrier(1, &barrier);
    using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Address = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    using Constants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
    xbox_method<Root>(list, 33)(list, root.Get());
    list->SetPipelineState(pso.Get());
    xbox_method<Address>(list, 45)(list, 0, output->GetGPUVirtualAddress());
    std::array<UINT, 56> constants{};
    const UINT dim = index == 0 ? 0 : 4 * (index + 1);
    constants[dim] = width;
    constants[dim + 1] = height;
    xbox_method<Constants>(list, 39)(list, 1, UINT(constants.size()), constants.data(), 0);
    // Include a completely inactive group after the partial final group.
    list->Dispatch((rows * columns - width * height + 31) / 32 + 1, 1, 1);
    barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(frame.readback.Get(), 0, output.Get(), 0, bytes);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;

    // Reference the tensor geometry directly, independently of the ISA's lane
    // indexing, division and predicate chain. Row-pitch gaps and interiors retain the guard.
    std::vector<UINT> expected(bytes / 4, guard);
    for (UINT p = 0; p < pass.planes; ++p) {
        for (UINT y = 0; y < rows; ++y) {
            for (UINT x = 0; x < columns; ++x) {
                if (y >= 1 && y <= height && x >= 1 && x <= width) continue;
                const UINT offset = pass.origin + p * pass.plane_stride + y * pass.pitch + x * 16;
                std::fill_n(expected.begin() + offset / 4, 4, 0);
            }
        }
    }
    CHECK(frame.readback->Map(0, nullptr, &mapped));
    const auto* got = static_cast<const UINT*>(mapped);
    auto mismatch = std::mismatch(expected.begin(), expected.end(), got);
    if (mismatch.first != expected.end())
        std::fprintf(stderr, "Pass %u offset %#zx expected %#x, read %#x\n",
                     index, size_t(mismatch.first - expected.begin()) * 4, *mismatch.first, *mismatch.second);
    frame.readback->Unmap(0, nullptr);
    return mismatch.first != expected.end();
}
}

int wmain(int argc, wchar_t** argv) {
    const wchar_t* path = argc == 2 ? argv[1] : argc == 1 ? _wgetenv(L"TOMBOX_FSR_LIBRARY") : nullptr;
    if (!path || !*path) { std::fprintf(stderr, "Usage: fsr_postpass_probe <upscaler.dll>\n"); return 2; }
    std::ifstream file(std::filesystem::path(path), std::ios::binary);
    if (!file) { std::fputs("Could not open the supplied upscaler library\n", stderr); return 1; }
    const std::vector<char> dll((std::istreambuf_iterator<char>(file)), {});
    auto passes = layout;
    for (size_t i = 0; i + 32 <= dll.size(); ++i) {
        if (std::memcmp(dll.data() + i, "DXBC", 4)) continue;
        UINT size;
        std::memcpy(&size, dll.data() + i + 24, 4);
        if (size < 32 || size > dll.size() - i) continue;
        uint64_t hash = 0xcbf29ce484222325ULL;
        for (UINT j = 0; j < size; ++j) hash = (hash ^ static_cast<uint8_t>(dll[i + j])) * 0x100000001b3ULL;
        for (auto& p : passes) if (p.hash == hash) p.code = {dll.data() + i, size};
    }
    for (UINT i = 0; i < passes.size(); ++i) {
        if (!passes[i].code.pShaderBytecode) { std::fprintf(stderr, "Missing FSR4 postpass %u\n", i); return 1; }
        const auto& pass = passes[i];
        const UINT max_width = pass.pitch / 16 - 1;
        const UINT max_height = pass.pitch == 0x3c20 ? 541 : pass.pitch == 0x1e20 ? 271 : 136;
        if (dispatch(pass, i, 43, 17, max_height) ||
            dispatch(pass, i, max_width, 2, max_height) ||
            dispatch(pass, i, 7, max_height, max_height)) return 1;
    }
    std::puts("13 retail FSR4 postpasses passed 39 border, interior and guard readbacks.");
    return 0;
}
