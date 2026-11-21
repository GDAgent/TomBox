// Exercise the translated retail kernels together. Read DXBC containers as data.
#include "graphics.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <numeric>
#include <vector>

namespace {
constexpr UINT groups = 8, count = 2048 * groups, words = count + 16;
constexpr UINT bytes = words * sizeof(UINT), guard = 0xcafe1234;
enum Buffer { keys, indices, prefix1, prefix2, out_keys, out_indices, buffers };

struct Kernel {
    const char* name;
    uint64_t hash;
    size_t size;
    D3D12_SHADER_BYTECODE code{};
    ComPtr<ID3D12PipelineState> pipeline;
};

template<size_t N>
int find_kernels(const std::vector<char>& dll, std::array<Kernel, N>& kernels) {
    for (size_t i = 0; i + 32 <= dll.size(); ++i) {
        if (std::memcmp(dll.data() + i, "DXBC", 4)) continue;
        UINT size;
        std::memcpy(&size, dll.data() + i + 24, sizeof(size));
        if (size > dll.size() - i) continue;
        for (auto& kernel : kernels) {
            if (size != kernel.size || kernel.code.pShaderBytecode) continue;
            uint64_t hash = 0xcbf29ce484222325ULL;
            for (UINT j = 0; j < size; ++j) hash = (hash ^ static_cast<uint8_t>(dll[i + j])) * 0x100000001b3ULL;
            if (hash == kernel.hash) kernel.code = {dll.data() + i, size};
        }
    }
    for (const auto& kernel : kernels) {
        if (!kernel.code.pShaderBytecode) {
            std::fprintf(stderr, "Missing %s kernel (%016llx) in the supplied library\n", kernel.name, kernel.hash);
            return 1;
        }
    }
    return 0;
}

int make_root(ID3D12Device* dev, UINT uavs, UINT constants, ID3D12RootSignature** root, bool source_srv = false) {
    std::vector<D3D12_ROOT_PARAMETER> params(uavs + 1);
    for (UINT i = 0; i < uavs; ++i) {
        params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        if (source_srv && i == 0) params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        params[i].Descriptor.ShaderRegister = source_srv && i > 0 ? i - 1 : i;
    }
    params.back().ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params.back().Constants = {0, 0, constants};
    D3D12_ROOT_SIGNATURE_DESC layout{UINT(params.size()), params.data(), 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    CHECK(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(root)));
    return 0;
}

void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* resource,
                D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {resource, 0, before, after};
    list->ResourceBarrier(1, &barrier);
}

// Vary the node count independently of the number of dispatch threads so the
// first bounds check, the backedge and its last partial wave all take both paths.
int copy_nodes(Kernel& kernel, bool source_srv, UINT nodes, UINT reset_bounds) {
    constexpr UINT threads = 64, capacity = 1600, byte_size = capacity * sizeof(UINT);
    const UINT copied = nodes * 16;
    Frame frame;
    if (frame.init(1, 1, false, byte_size)) return 1;
    auto* dev = frame.dev.Get();
    auto* list = frame.list.Get();
    ComPtr<ID3D12RootSignature> root;
    if (make_root(dev, 2, 16, &root, source_srv)) return 1;
    ComPtr<ID3D12PipelineState> pipeline;
    D3D12_COMPUTE_PIPELINE_STATE_DESC pso{};
    pso.pRootSignature = root.Get();
    pso.CS = kernel.code;
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                     UINT, const void*, REFIID, void**);
    CHECK(xbox_method<Create>(dev, 62)(dev, &pso, 0, nullptr, IID_PPV_ARGS(&pipeline)));

    auto desc = frame.readback->GetDesc();
    desc.Width = byte_size * 2;
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> upload, source, dest;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    std::vector<UINT> initial(capacity * 2, guard);
    for (UINT i = 0; i < capacity; ++i) initial[i] = i * 2246822519u + 67u;
    initial[1] = nodes - 1;
    void* ptr;
    CHECK(upload->Map(0, nullptr, &ptr));
    std::memcpy(ptr, initial.data(), size_t(desc.Width));
    upload->Unmap(0, nullptr);
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    desc.Width = byte_size;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    for (auto* data : {source.GetAddressOf(), dest.GetAddressOf()}) {
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                           nullptr, IID_PPV_ARGS(data)));
    }
    const auto source_state = source_srv ? D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE : D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    list->CopyBufferRegion(source.Get(), 0, upload.Get(), 0, byte_size);
    list->CopyBufferRegion(dest.Get(), 0, upload.Get(), byte_size, byte_size);
    transition(list, source.Get(), D3D12_RESOURCE_STATE_COPY_DEST, source_state);
    transition(list, dest.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Address = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    using Constants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
    xbox_method<Root>(list, 33)(list, root.Get());
    list->SetPipelineState(pipeline.Get());
    xbox_method<Address>(list, source_srv ? 43 : 45)(list, 0, source->GetGPUVirtualAddress());
    xbox_method<Address>(list, 45)(list, 1, dest->GetGPUVirtualAddress());
    UINT values[16] = {threads, reset_bounds};
    xbox_method<Constants>(list, 39)(list, 2, 16, values, 0);
    list->Dispatch(threads / 32, 1, 1);
    transition(list, dest.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(frame.readback.Get(), 0, dest.Get(), 0, byte_size);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;
    CHECK(frame.readback->Map(0, nullptr, &ptr));
    bool ok = true;
    for (UINT i = 0; i < capacity; ++i) {
        UINT expected = i < copied ? initial[i] : guard;
        if (reset_bounds && i >= 4 && i <= 9) expected = i < 7 ? 0x7f7fffffu : 0xff7fffffu;
        if (static_cast<const UINT*>(ptr)[i] != expected) {
            std::fprintf(stderr, "%s nodes=%u reset=%u word %u expected %08x got %08x\n", kernel.name,
                         nodes, reset_bounds, i, expected, static_cast<const UINT*>(ptr)[i]);
            ok = false;
            break;
        }
    }
    frame.readback->Unmap(0, nullptr);
    return ok ? 0 : 1;
}

int sort(std::array<Kernel, 4>& kernels, void (*order)(ID3D12GraphicsCommandList*)) {
    Frame frame;
    if (frame.init(1, 1, false, 8 * bytes)) return 1;
    auto* dev = frame.dev.Get();
    auto* list = frame.list.Get();
    ComPtr<ID3D12RootSignature> radix_root, scan_root;
    if (make_root(dev, 6, 3, &radix_root) || make_root(dev, 2, 1, &scan_root)) return 1;
    auto& histogram = kernels[0];
    auto& block_scan = kernels[1];
    auto& totals_scan = kernels[2];
    auto& scatter = kernels[3];
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                     UINT, const void*, REFIID, void**);
    for (auto& kernel : kernels) {
        D3D12_COMPUTE_PIPELINE_STATE_DESC pso{};
        pso.CS = kernel.code;
        pso.pRootSignature = (&kernel == &histogram || &kernel == &scatter) ? radix_root.Get() : scan_root.Get();
        CHECK(xbox_method<Create>(dev, 62)(dev, &pso, 0, nullptr, IID_PPV_ARGS(&kernel.pipeline)));
    }

    // Repeated keys make an unstable permutation visible in the index output.
    std::vector<UINT> initial(buffers * words, guard);
    for (UINT i = 0; i < count; ++i) {
        UINT bucket = ((i * 2654435761u) >> 21) % 1021;
        initial[keys * words + i] = (bucket * 2246822519u) ^ 0x3861e4d3u;
        initial[indices * words + i] = i;
    }
    std::array<ComPtr<ID3D12Resource>, buffers> data;
    ComPtr<ID3D12Resource> upload;
    auto desc = frame.readback->GetDesc();
    desc.Width = initial.size() * sizeof(UINT);
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* ptr;
    CHECK(upload->Map(0, nullptr, &ptr));
    std::memcpy(ptr, initial.data(), size_t(desc.Width));
    upload->Unmap(0, nullptr);
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    desc.Width = bytes;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    for (UINT i = 0; i < buffers; ++i) {
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                           nullptr, IID_PPV_ARGS(&data[i])));
        list->CopyBufferRegion(data[i].Get(), 0, upload.Get(), i * bytes, bytes);
        transition(list, data[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    }
    auto dispatch = [&](Kernel& kernel, std::initializer_list<Buffer> bindings,
                        std::initializer_list<UINT> constants, UINT group_count) {
        using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
        using Address = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
        using Constants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
        xbox_method<Root>(list, 33)(list, (&kernel == &histogram || &kernel == &scatter) ? radix_root.Get() : scan_root.Get());
        list->SetPipelineState(kernel.pipeline.Get());
        UINT slot = 0;
        for (auto binding : bindings) xbox_method<Address>(list, 45)(list, slot++, data[binding]->GetGPUVirtualAddress());
        xbox_method<Constants>(list, 39)(list, slot, UINT(constants.size()), constants.begin(), 0);
        list->Dispatch(group_count, 1, 1);
        order(list);
    };
    for (UINT pass = 0; pass < 4; ++pass) {
        dispatch(histogram, {keys, prefix1, indices, prefix2, out_keys, out_indices}, {groups, pass * 8, 10}, groups);
        // Each 1,024-counter scan writes one block total. Scan those totals in place.
        dispatch(block_scan, {prefix1, prefix2}, {1}, (256 * groups) / 1024);
        dispatch(totals_scan, {prefix2, prefix1}, {0}, 1);
        dispatch(scatter, {keys, indices, prefix1, prefix2, out_keys, out_indices}, {groups, pass * 8, 10}, groups);
        for (UINT i = 0; i < 2; ++i) {
            auto* src = data[i == 0 ? out_keys : out_indices].Get();
            transition(list, src, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
            list->CopyBufferRegion(frame.readback.Get(), (2 * pass + i) * bytes, src, 0, bytes);
            transition(list, src, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        }
        data[keys].Swap(data[out_keys]);
        data[indices].Swap(data[out_indices]);
    }
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;
    CHECK(frame.readback->Map(0, nullptr, &ptr));
    const auto* result = static_cast<const UINT*>(ptr);
    for (UINT pass = 0; pass < 4; ++pass) {
        std::vector<UINT> permutation(count);
        std::iota(permutation.begin(), permutation.end(), 0u);
        const UINT mask = UINT_MAX >> ((3 - pass) * 8);
        std::stable_sort(permutation.begin(), permutation.end(), [&](UINT a, UINT b) {
            return (initial[a] & mask) < (initial[b] & mask);
        });
        for (UINT i = 0; i < words; ++i) {
            const UINT key = i < count ? initial[permutation[i]] : guard;
            const UINT index = i < count ? permutation[i] : guard;
            if (result[2 * pass * words + i] != key || result[(2 * pass + 1) * words + i] != index) {
                std::fprintf(stderr, "BVH sort mismatch on byte %u at word %u\n", pass, i);
                frame.readback->Unmap(0, nullptr);
                return 1;
            }
        }
    }
    frame.readback->Unmap(0, nullptr);
    std::printf("BVH radix sort verified %u key/index pairs and both output guards after each of four passes.\n", count);
    return 0;
}
}

int wmain(int argc, wchar_t** argv) {
    const wchar_t* path = argc == 2 ? argv[1] : argc == 1 ? _wgetenv(L"TOMBOX_BVH_LIBRARY") : nullptr;
    if (!path || !*path) {
        std::fputs("Usage: bvh_sort_probe.exe path-to-XboxBvhBuilder.dll\n", stderr);
        return 2;
    }
    std::ifstream stream(std::filesystem::path(path), std::ios::binary);
    if (!stream) { std::fputs("Could not open the supplied BVH library\n", stderr); return 1; }
    std::vector<char> dll((std::istreambuf_iterator<char>(stream)), {});
    std::array<Kernel, 4> kernels{{
        {"histogram", 0xdf7efd69ef9e6a32ULL, 8188},
        {"multi-wave prefix scan", 0x3bda8b7a22dff406ULL, 5700},
        {"block totals scan", 0xca71137d2e45e851ULL, 5072},
        {"scatter", 0xfa8828838850c8e0ULL, 16184},
    }};
    std::array<Kernel, 2> copies{{
        {"BVH UAV copy", 0x9e3107c7a215e9e9ULL, 4404},
        {"BVH SRV copy", 0x83225f64b531e807ULL, 4716},
    }};
    if (find_kernels(dll, kernels) || find_kernels(dll, copies)) return 1;
    for (UINT nodes : {3u, 87u}) {
        if (copy_nodes(copies[0], false, nodes, 0) || copy_nodes(copies[1], true, nodes, 0)
            || copy_nodes(copies[1], true, nodes, 1)) return 1;
    }
    std::puts("BVH strided copies verified both source bindings, node-bounds reset and destination guards.");
    HMODULE module = LoadLibraryW(L"bvh_commands.dll");
    auto order = module ? reinterpret_cast<void (*)(ID3D12GraphicsCommandList*)>(
        GetProcAddress(module, "order_compute_writes")) : nullptr;
    if (!order) { std::fputs("Missing homebrew BVH command-list fixture\n", stderr); return 1; }
    int result = sort(kernels, order);
    FreeLibrary(module);
    return result;
}
