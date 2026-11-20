#pragma once

#include <array>
#include <fstream>
#include <string>
#include <vector>

enum class ComputeProbe { OfflineFallback, UavBarrier, ThreadDispatch, VectorAlu, PackedRoot32, PackedRoot64 };
using VectorRow = std::array<UINT, 8>; // v0, v1, v2, initial VCC; result, VCC, EXEC, marker

static int run_compute(Frame& frame, ComputeProbe probe, D3D12_SHADER_BYTECODE shader, UINT groups,
                       VectorRow* rows = nullptr) {
    const bool test_dispatch = probe == ComputeProbe::ThreadDispatch;
    const bool packed = probe == ComputeProbe::PackedRoot32 || probe == ComputeProbe::PackedRoot64;
    auto* dev = frame.dev.Get();
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER params[2]{};
    if (packed) {
        params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
        params[1].Constants = {0, 0, 4};
    } else {
        params[0].DescriptorTable = {1, &range};
    }
    D3D12_ROOT_SIGNATURE_DESC layout{packed ? 2u : 1u, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
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
    const UINT elements = groups * (probe == ComputeProbe::PackedRoot64 ? 64 : packed ? 32 : 1);
    buffer.Width = UINT64(elements) * stride;
    buffer.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> output;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
          rows ? D3D12_RESOURCE_STATE_COPY_DEST : D3D12_RESOURCE_STATE_UNORDERED_ACCESS, nullptr, IID_PPV_ARGS(&output)));
    ComPtr<ID3D12DescriptorHeap> views;
    D3D12_GPU_DESCRIPTOR_HANDLE gpu{};
    if (!packed) {
        D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1,
                                      D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
        CHECK(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&views)));
        using Handle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
        D3D12_CPU_DESCRIPTOR_HANDLE cpu{xbox_method<Handle>(views.Get(), 9)(views.Get())};
        gpu = {xbox_method<Handle>(views.Get(), 10)(views.Get())};
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
        uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        uav.Buffer.NumElements = groups;
        uav.Buffer.StructureByteStride = stride;
        dev->CreateUnorderedAccessView(output.Get(), nullptr, &uav, cpu);
    }

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
        xbox_method<Root>(list, 33)(list, root.Get());
        if (packed) {
            using Address = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
            using Constants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
            const UINT values[] = {71, 0xf8000047u, 0, 0};
            xbox_method<Address>(list, 45)(list, 0, output->GetGPUVirtualAddress());
            xbox_method<Constants>(list, 39)(list, 1, 4, values, 0);
        } else {
            xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
            xbox_method<Table>(list, 35)(list, 0, gpu);
        }
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

// Check every word, including the last group's inactive lanes and the input column.
static int check_packed_root() {
    std::array<VectorRow, 96> rows;
    for (UINT i = 0; i < rows.size(); ++i) {
        rows[i].fill(0x5a5a5a5a);
        rows[i][1] = i * 0x142135u;
    }
    auto expected = rows;
    for (UINT i = 0; i < 71; ++i) expected[i][4] = expected[i][1] + 0xf8000047u;
    Frame frame;
    if (frame.init(8, 4, false, sizeof(rows)) ||
        run_compute(frame, ComputeProbe::PackedRoot32, {packed_root_cs, sizeof(packed_root_cs)}, 3, rows.data())) return 1;
    if (rows != expected) {
        std::fputs("Packed root buffer readback mismatch\n", stderr);
        return 1;
    }
    return 0;
}

// Compare both ballot words and leave sentinel values in every masked lane.
static int check_packed_ballot() {
    std::array<VectorRow, 128> rows;
    for (UINT i = 0; i < rows.size(); ++i) {
        rows[i].fill(0xccccccccu);
        rows[i][1] = (i * 37 % 11) < 5;
    }
    auto expected = rows;
    for (UINT group = 0; group < 2; ++group) {
        UINT bits[2]{};
        for (UINT lane = 0; lane < 57; ++lane) {
            if (rows[group * 64 + lane][1]) bits[lane / 32] |= 1u << (lane % 32);
        }
        for (UINT lane = 0; lane < 57; ++lane) {
            expected[group * 64 + lane][4] = bits[0] ^ lane;
            expected[group * 64 + lane][5] = bits[1] ^ lane;
        }
    }
    Frame frame;
    if (frame.init(8, 4, false, sizeof(rows)) ||
        run_compute(frame, ComputeProbe::PackedRoot64, {packed_ballot_cs, sizeof(packed_ballot_cs)}, 2, rows.data())) return 1;
    if (rows != expected) {
        std::fputs("Packed wave64 ballot readback mismatch\n", stderr);
        return 1;
    }
    return 0;
}

static int check_packed_scan(UINT width) {
    std::vector<VectorRow> rows(3 * width);
    for (UINT i = 0; i < rows.size(); ++i) {
        rows[i].fill(0x5a5a5a5a);
        rows[i][1] = (i + 1) * 0x85ebca6bu;
    }
    auto expected = rows;
    for (UINT group = 0; group < 3; ++group) {
        UINT sum = 0;
        for (UINT lane = 0; lane < width; ++lane) {
            auto& row = expected[group * width + lane];
            sum += row[1];
            row[4] = sum;
        }
    }
    Frame frame;
    const auto shader = width == 32 ? D3D12_SHADER_BYTECODE{packed_scan32_cs, sizeof(packed_scan32_cs)}
                                    : D3D12_SHADER_BYTECODE{packed_scan64_cs, sizeof(packed_scan64_cs)};
    const auto probe = width == 32 ? ComputeProbe::PackedRoot32 : ComputeProbe::PackedRoot64;
    if (frame.init(8, 4, false, rows.size() * sizeof(VectorRow)) ||
        run_compute(frame, probe, shader, 3, rows.data())) return 1;
    if (rows != expected) {
        std::fprintf(stderr, "Packed wave%u scan readback mismatch\n", width);
        return 1;
    }
    return 0;
}

static int check_packed_quad() {
    std::array<VectorRow, 96> rows;
    auto bits = [](float value) { UINT u; std::memcpy(&u, &value, sizeof(u)); return u; };
    auto input = [](UINT i) { return float(i * i + 1); };
    auto first = [&](UINT i) { return i < 71 ? input(i ^ 1) + input(i) : input(i); };
    for (UINT i = 0; i < rows.size(); ++i) {
        rows[i].fill(0xdeadbeef);
        rows[i][1] = bits(input(i));
    }
    auto expected = rows;
    for (UINT i = 0; i < 71; ++i) {
        expected[i][4] = bits(first(i));
        expected[i][5] = bits(first(i ^ 2) + first(i));
    }
    Frame frame;
    if (frame.init(8, 4, false, sizeof(rows)) ||
        run_compute(frame, ComputeProbe::PackedRoot32, {packed_quad_cs, sizeof(packed_quad_cs)}, 3, rows.data())) return 1;
    if (rows != expected) {
        std::fputs("FP32 DPP quad add readback mismatch\n", stderr);
        return 1;
    }
    return 0;
}

static int check_retiring_wave() {
    std::array<VectorRow, 3 * 64> rows;
    for (UINT i = 0; i < rows.size(); ++i) {
        rows[i].fill(0xdeadbeef);
        rows[i][1] = 0xf37a5900u + 17 * i;
    }
    auto expected = rows;
    for (UINT i = 0; i < rows.size(); ++i) {
        if (i % 64 < 20) expected[i][4] = rows[i + 32][1];
    }
    Frame frame;
    if (frame.init(8, 4, false, sizeof(rows)) ||
        run_compute(frame, ComputeProbe::PackedRoot64, {retiring_wave_cs, sizeof(retiring_wave_cs)}, 3, rows.data())) return 1;
    if (rows != expected) {
        std::fputs("LDS readback after early wave exit failed\n", stderr);
        return 1;
    }
    return 0;
}

static int check_packed_compute() {
    if (check_packed_root() || check_packed_quad() || check_retiring_wave() || check_packed_scan(32)) return 1;
    if (!GetEnvironmentVariableA("TOMBOX_PROBE_WAVE64", nullptr, 0)) return 0;
    return check_packed_ballot() || check_packed_scan(64);
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
