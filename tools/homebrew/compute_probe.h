#pragma once

#include <array>
#include <fstream>
#include <string>
#include <vector>

enum class ComputeProbe { OfflineFallback, UavBarrier, ThreadDispatch, VectorAlu };
using VectorRow = std::array<UINT, 8>; // v0, v1, v2, initial VCC; result, VCC, EXEC, marker

static int run_compute(Frame& frame, ComputeProbe probe, D3D12_SHADER_BYTECODE shader, UINT groups,
                       VectorRow* rows = nullptr) {
    const bool test_dispatch = probe == ComputeProbe::ThreadDispatch;
    auto* dev = frame.dev.Get();
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER param{};
    param.DescriptorTable = {1, &range};
    D3D12_ROOT_SIGNATURE_DESC layout{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    CHECK(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    ComPtr<ID3D12RootSignature> root;
    CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)));

    if (probe == ComputeProbe::OfflineFallback) {
        using Deserialize = HRESULT (WINAPI*)(ID3D12Device*, const void*, REFIID, void**);
        const void* offline[9] = {root.Get()};
        void* result = root.Get(); // Failure must clear a previously populated out parameter.
        HRESULT hr = xbox_method<Deserialize>(dev, 67)(dev, offline, __uuidof(ID3D12PipelineState), &result);
        if (hr != E_NOTIMPL || result != nullptr) {
            std::fprintf(stderr, "Offline compute PSO returned %#lx, output %p\n", hr, result);
            return 1;
        }
    }
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                     UINT, const void*, REFIID, void**);
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = shader;
    ComPtr<ID3D12PipelineState> pso;
    CHECK(xbox_method<Create>(dev, 62)(dev, &desc, 0, nullptr, IID_PPV_ARGS(&pso)));
    if (!pso) return 1;

    const UINT stride = rows ? sizeof(VectorRow) : sizeof(UINT);
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    auto buffer = frame.readback->GetDesc();
    buffer.Width = UINT64(groups) * stride;
    buffer.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> output;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
          rows ? D3D12_RESOURCE_STATE_COPY_DEST : D3D12_RESOURCE_STATE_UNORDERED_ACCESS, nullptr, IID_PPV_ARGS(&output)));
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> views;
    CHECK(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&views)));
    using Handle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
    D3D12_CPU_DESCRIPTOR_HANDLE cpu{xbox_method<Handle>(views.Get(), 9)(views.Get())};
    D3D12_GPU_DESCRIPTOR_HANDLE gpu{xbox_method<Handle>(views.Get(), 10)(views.Get())};
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Buffer.NumElements = groups;
    uav.Buffer.StructureByteStride = stride;
    dev->CreateUnorderedAccessView(output.Get(), nullptr, &uav, cpu);

    auto* list = frame.list.Get();
    using Heaps = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    ComPtr<ID3D12Resource> upload;
    if (rows) {
        heap.Type = D3D12_HEAP_TYPE_UPLOAD;
        auto source = buffer;
        source.Flags = D3D12_RESOURCE_FLAG_NONE;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &source,
              D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&upload)));
        void* mapped = nullptr;
        D3D12_RANGE no_reads{};
        CHECK(upload->Map(0, &no_reads, &mapped));
        std::memcpy(mapped, rows, size_t(buffer.Width));
        upload->Unmap(0, nullptr);
        list->CopyBufferRegion(output.Get(), 0, upload.Get(), 0, buffer.Width);
        D3D12_RESOURCE_BARRIER ready{};
        ready.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS};
        list->ResourceBarrier(1, &ready);
    }
    auto bind_inputs = [&] {
        xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
        xbox_method<Root>(list, 33)(list, root.Get());
        xbox_method<Table>(list, 35)(list, 0, gpu);
    };
    auto submit = [&]() -> int {
        CHECK(list->Close());
        ID3D12CommandList* lists[] = {list};
        frame.queue->ExecuteCommandLists(1, lists);
        return frame.wait_for_gpu();
    };
    bind_inputs();
    list->SetPipelineState(pso.Get());
    list->Dispatch(groups, 1, 1);
    ComPtr<ID3D12PipelineState> increment;
    ComPtr<ID3D12GraphicsCommandList> second;
    // Keep both passes in UAV state. The second shader consumes the first one's writes.
    if (probe == ComputeProbe::UavBarrier || test_dispatch) {
        desc.CS = test_dispatch ? D3D12_SHADER_BYTECODE{increment4_cs, sizeof(increment4_cs)}
                                : D3D12_SHADER_BYTECODE{increment_cs, sizeof(increment_cs)};
        CHECK(xbox_method<Create>(dev, 62)(dev, &desc, 0, nullptr, IID_PPV_ARGS(&increment)));
        list->SetPipelineState(increment.Get());
        HMODULE module = LoadLibraryW(L"bvh_commands.dll");
        using Order = void (*)(ID3D12GraphicsCommandList*);
        using Threads = void (*)(ID3D12GraphicsCommandList*, UINT, UINT);
        auto order = module ? reinterpret_cast<Order>(GetProcAddress(module, "order_compute_writes")) : nullptr;
        auto dispatch = module ? reinterpret_cast<Threads>(GetProcAddress(module, "dispatch_threads")) : nullptr;
        if (!order || (test_dispatch && !dispatch)) {
            std::fputs("Could not load the 250405 compute fixture\n", stderr);
            if (module) FreeLibrary(module);
            return 1;
        }
        order(list);
        if (test_dispatch) {
            dispatch(list, 16, 0x20);
            order(list);
            // ClearState switches to the one-thread writer. It overwrites just two elements.
            list->ClearState(pso.Get());
            bind_inputs();
            dispatch(list, 2, 0x20);
            order(list);
            if (submit()) { FreeLibrary(module); return 1; }
            // Reset back to four-thread groups, using the initial PSO argument alone.
            HRESULT hr = list->Reset(frame.alloc.Get(), increment.Get());
            if (FAILED(hr)) { FreeLibrary(module); return 1; }
            bind_inputs();
            dispatch(list, 8, 0x8020);
            order(list);
            dispatch(list, 7, 0x20);    // Partial last group.
            dispatch(list, 16, 0x21);   // Unverified initiator bits.
            if (submit() || frame.create_list(second.GetAddressOf(), increment.Get())) {
                FreeLibrary(module);
                return 1;
            }
            list = second.Get();
            bind_inputs();
            dispatch(list, 4, 0x20);
        } else {
            list->Dispatch(groups, 1, 1);
        }
        FreeLibrary(module);
    }
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(frame.readback.Get(), 0, output.Get(), 0, buffer.Width);
    if (submit()) return 1;

    UINT* data = nullptr;
    D3D12_RANGE read{0, size_t(buffer.Width)}, no_writes{};
    CHECK(frame.readback->Map(0, &read, reinterpret_cast<void**>(&data)));
    if (rows) {
        std::memcpy(rows, data, size_t(buffer.Width));
        frame.readback->Unmap(0, &no_writes);
        return 0;
    }
    UINT matched = 0;
    for (UINT i = 0; i < groups; ++i) {
        UINT expected = 0x57ac71a9u;
        if (test_dispatch) expected += (i >= 2 && i < 16) + (i < 8) + (i < 4);
        if (probe == ComputeProbe::UavBarrier) ++expected;
        matched += data[i] == expected;
    }
    frame.readback->Unmap(0, &no_writes);
    const char* label = test_dispatch ? "Thread dispatch" :
        (probe == ComputeProbe::UavBarrier ? "Compute UAV barrier" : "Compute PSO fallback");
    std::printf("%s: %u/%u values match\n", label, matched, groups);
    return matched != groups;
}

static int check_compute(ComputeProbe probe) {
    Frame frame;
    if (frame.init(8, 4)) return 1;
    return run_compute(frame, probe, {fallback_cs, sizeof(fallback_cs)}, 32);
}

// The launcher generates a suite of stripped Xbox compute shaders and source registers.
// Keep one Xbox device alive and fence each dispatch before reading its result.
static int check_rdna2(const char* directory) {
    const std::string dir(directory);
    std::ifstream suite(dir + "/suite.bin", std::ios::binary);
    std::ofstream results(dir + "/results.bin", std::ios::binary);
    UINT count = 0;
    if (!suite.read(reinterpret_cast<char*>(&count), sizeof(count)) || !count || !results) return 1;
    constexpr UINT max_groups = D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION;
    Frame frame;
    if (frame.init(8, 4, false, max_groups * sizeof(VectorRow))) return 1;
    for (UINT i = 0; i < count; ++i) {
        UINT header[2]{};
        if (!suite.read(reinterpret_cast<char*>(header), sizeof(header))) return 1;
        const auto shader_bytes = header[0], groups = header[1];
        if (!shader_bytes || shader_bytes > (1u << 20) || !groups || groups > max_groups) return 1;
        std::vector<char> shader(shader_bytes);
        std::vector<VectorRow> rows(groups);
        if (!suite.read(shader.data(), shader_bytes) ||
            !suite.read(reinterpret_cast<char*>(rows.data()), groups * sizeof(VectorRow))) return 1;
        if (i) {
            CHECK(frame.alloc->Reset());
            CHECK(frame.list->Reset(frame.alloc.Get(), nullptr));
        }
        if (run_compute(frame, ComputeProbe::VectorAlu, {shader.data(), shader.size()}, groups, rows.data())) {
            std::fprintf(stderr, "Coin Run RDNA2 case %u failed\n", i);
            return 1;
        }
        for (const auto& row : rows) {
            results.write(reinterpret_cast<const char*>(row.data() + 4), 4 * sizeof(UINT));
        }
        if (!results) return 1;
    }
    results.close();
    return !results || suite.peek() != std::ifstream::traits_type::eof();
}
