#pragma once

// Reproduce the ACO failure -> ordinary compute PSO path used by the upscaler.
static int check_compute_fallback() {
    Frame frame;
    if (frame.init(8, 4)) return 1;
    auto* dev = frame.dev.Get();
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER param{};
    param.DescriptorTable = {1, &range};
    D3D12_ROOT_SIGNATURE_DESC layout{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    CHECK(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    ComPtr<ID3D12RootSignature> root;
    CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)));

    using Deserialize = HRESULT (WINAPI*)(ID3D12Device*, const void*, REFIID, void**);
    const void* offline[9] = {root.Get()};
    void* result = root.Get(); // Failure must clear a previously populated out parameter.
    HRESULT hr = xbox_method<Deserialize>(dev, 67)(dev, offline, __uuidof(ID3D12PipelineState), &result);
    if (hr != E_NOTIMPL || result != nullptr) {
        std::fprintf(stderr, "Offline compute PSO returned %#lx, output %p\n", hr, result);
        return 1;
    }
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                     UINT, const void*, REFIID, void**);
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {fallback_cs, sizeof(fallback_cs)};
    ComPtr<ID3D12PipelineState> pso;
    CHECK(xbox_method<Create>(dev, 62)(dev, &desc, 0, nullptr, IID_PPV_ARGS(&pso)));
    if (!pso) return 1;

    constexpr UINT groups = 32;
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    auto buffer = frame.readback->GetDesc();
    buffer.Width = groups * sizeof(UINT);
    buffer.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> output;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
          D3D12_RESOURCE_STATE_UNORDERED_ACCESS, nullptr, IID_PPV_ARGS(&output)));
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
    uav.Buffer.StructureByteStride = sizeof(UINT);
    dev->CreateUnorderedAccessView(output.Get(), nullptr, &uav, cpu);

    auto* list = frame.list.Get();
    using Heaps = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
    xbox_method<Root>(list, 33)(list, root.Get());
    xbox_method<Table>(list, 35)(list, 0, gpu);
    list->SetPipelineState(pso.Get());
    list->Dispatch(groups, 1, 1);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    list->CopyBufferRegion(frame.readback.Get(), 0, output.Get(), 0, buffer.Width);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;

    UINT* data = nullptr;
    D3D12_RANGE read{0, size_t(buffer.Width)}, no_writes{};
    CHECK(frame.readback->Map(0, &read, reinterpret_cast<void**>(&data)));
    UINT matched = 0;
    for (UINT i = 0; i < groups; ++i) matched += data[i] == 0x57ac71a9;
    frame.readback->Unmap(0, &no_writes);
    std::printf("Compute PSO fallback: %u/%u group writes match\n", matched, groups);
    return matched != groups;
}
