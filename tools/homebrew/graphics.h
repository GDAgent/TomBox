#pragma once
#include <windows.h>
#include <d3d12.h>
#include <wrl/client.h>
#include <cstdio>
#include <cstring>
using Microsoft::WRL::ComPtr;

extern "C" __declspec(dllimport) HRESULT WINAPI D3D12XboxCreateDevice(
    void*, const void*, REFIID, void**);

template<typename Fn, typename Object>
Fn xbox_method(Object* object, unsigned slot) {
    return reinterpret_cast<Fn>((*reinterpret_cast<void***>(object))[slot]);
}

#define CHECK(call) do { HRESULT hr = (call); if (FAILED(hr)) { \
    std::fprintf(stderr, "%s failed (HRESULT 0x%08lx)\n", #call, hr); return 1; \
} } while (0)

struct Frame {
    ComPtr<ID3D12Device> dev;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Resource> target;
    ComPtr<ID3D12DescriptorHeap> views;
    ComPtr<ID3D12Resource> readback;
    ComPtr<ID3D12Fence> fence;
    ComPtr<ID3D12Resource> depth;
    ComPtr<ID3D12DescriptorHeap> depth_views;
    D3D12_CPU_DESCRIPTOR_HANDLE rtv{}, dsv{};
    UINT width, height, pitch;
    UINT64 frame = 0, presents = 0;

    int init(UINT w, UINT h, bool with_depth = false) {
        width = w;
        height = h;
        pitch = (width * 4 + 255) & ~255u;
        CHECK(D3D12XboxCreateDevice(nullptr, nullptr, IID_PPV_ARGS(&dev)));
        D3D12_COMMAND_QUEUE_DESC qdesc = {};
        using QueueX = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMMAND_QUEUE_DESC*, REFIID, void**);
        CHECK(xbox_method<QueueX>(dev.Get(), 48)(dev.Get(), &qdesc, IID_PPV_ARGS(&queue)));
        CHECK(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)));
        if (create_list(list.GetAddressOf())) return 1;

        D3D12_HEAP_PROPERTIES heap = {};
        heap.Type = D3D12_HEAP_TYPE_DEFAULT;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC desc = {};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        desc.Width = width;
        desc.Height = height;
        desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
        desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
              D3D12_RESOURCE_STATE_RENDER_TARGET, nullptr, IID_PPV_ARGS(&target)));

        D3D12_DESCRIPTOR_HEAP_DESC hdesc = {D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1};
        CHECK(dev->CreateDescriptorHeap(&hdesc, IID_PPV_ARGS(&views)));
        // TomBox returns this handle directly in RAX (Xbox ABI).
        using CpuHandle = SIZE_T (WINAPI*)(ID3D12DescriptorHeap*);
        rtv = {xbox_method<CpuHandle>(views.Get(), 9)(views.Get())};
        dev->CreateRenderTargetView(target.Get(), nullptr, rtv);

        if (with_depth) {
            desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
            desc.Format = DXGI_FORMAT_D32_FLOAT;
            D3D12_CLEAR_VALUE clear{};
            clear.Format = desc.Format;
            clear.DepthStencil.Depth = 1;
            CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
                  D3D12_RESOURCE_STATE_DEPTH_WRITE, &clear, IID_PPV_ARGS(&depth)));
            hdesc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_DSV;
            CHECK(dev->CreateDescriptorHeap(&hdesc, IID_PPV_ARGS(&depth_views)));
            dsv = {xbox_method<CpuHandle>(depth_views.Get(), 9)(depth_views.Get())};
            dev->CreateDepthStencilView(depth.Get(), nullptr, dsv);
        }

        heap.Type = D3D12_HEAP_TYPE_READBACK;
        desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        desc.Width = height * pitch;
        desc.Height = 1;
        desc.Format = DXGI_FORMAT_UNKNOWN;
        desc.Flags = D3D12_RESOURCE_FLAG_NONE;
        desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
              D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&readback)));

        CHECK(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)));
        return 0;
    }

    int create_list(ID3D12GraphicsCommandList** out) {
        const UINT params[4] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
        using ListX = HRESULT (WINAPI*)(ID3D12Device*, const UINT*, ID3D12CommandAllocator*,
                                        ID3D12PipelineState*, REFIID, void**);
        CHECK(xbox_method<ListX>(dev.Get(), 73)(dev.Get(), params, alloc.Get(), nullptr, IID_PPV_ARGS(out)));
        return 0;
    }

    void transition(D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
        D3D12_RESOURCE_BARRIER barrier = {};
        barrier.Transition = {target.Get(), 0, before, after};
        list->ResourceBarrier(1, &barrier);
    }

    int begin(const float (&color)[4]) {
        if (frame) {
            CHECK(alloc->Reset());
            CHECK(list->Reset(alloc.Get(), nullptr));
            transition(D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_RENDER_TARGET);
        }
        using ClearX = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                                      const float*, UINT, const D3D12_RECT*);
        xbox_method<ClearX>(list.Get(), 52)(list.Get(), rtv, color, 0, nullptr);
        if (depth) {
            using ClearDepthX = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                                               D3D12_CLEAR_FLAGS, float, UINT8, UINT, const D3D12_RECT*);
            xbox_method<ClearDepthX>(list.Get(), 51)(list.Get(), dsv, D3D12_CLEAR_FLAG_DEPTH, 1, 0, 0, nullptr);
        }
        return 0;
    }

    int present_target() {
        // PresentX expects a plane with its resource count at 8 and its array at 16.
        ID3D12Resource* resource = target.Get();
        struct Plane { UINT64 token; UINT count; UINT padding; ID3D12Resource** resources; };
        Plane plane = {++presents, 1, 0, &resource};
        using PresentX = HRESULT (WINAPI*)(ID3D12CommandQueue*, UINT, const Plane*, const void*);
        CHECK(xbox_method<PresentX>(queue.Get(), 30)(queue.Get(), 1, &plane, nullptr));
        return 0;
    }

    // Copy one frame-sized, four-byte-per-pixel texture already in COPY_SOURCE.
    void copy_to_readback(ID3D12Resource* resource, DXGI_FORMAT format) {
        D3D12_TEXTURE_COPY_LOCATION dst = {}, src = {};
        dst.pResource = readback.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        dst.PlacedFootprint.Footprint = {format, width, height, 1, pitch};
        src.pResource = resource;
        src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        using CopyX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                     UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
        xbox_method<CopyX>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
    }

    int finish(bool present = false) {
        transition(D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE);
        copy_to_readback(target.Get(), DXGI_FORMAT_R8G8B8A8_UNORM);
        transition(D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON);
        CHECK(list->Close());
        ID3D12CommandList* work[] = {list.Get()};
        queue->ExecuteCommandLists(1, work);
        if (present && present_target()) return 1;
        return wait_for_gpu();
    }

    int wait_for_gpu() {
        CHECK(queue->Signal(fence.Get(), ++frame));
        HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        if (!done) return 1;
        HRESULT armed = fence->SetEventOnCompletion(frame, done);
        DWORD wait = SUCCEEDED(armed) ? WaitForSingleObject(done, 10000) : WAIT_FAILED;
        CloseHandle(done);
        if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != frame) {
            std::fprintf(stderr, "GPU fence failed (wait=%lu, event HRESULT=0x%08lx)\n", wait, armed);
            return 1;
        }
        return 0;
    }
};

// Keep GPU objects alive until the queue completes.
struct Mesh {
    ComPtr<ID3D12Resource> vertices;
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pipeline;
    UINT capacity_bytes = 0;

    int init(ID3D12Device* dev, D3D12_SHADER_BYTECODE vs, D3D12_SHADER_BYTECODE ps,
             const D3D12_INPUT_ELEMENT_DESC* inputs, UINT input_count, UINT bytes,
             const D3D12_ROOT_PARAMETER* tables = nullptr, UINT table_count = 0, bool with_depth = false) {
        D3D12_ROOT_SIGNATURE_DESC sig = {};
        sig.NumParameters = table_count;
        sig.pParameters = tables;
        sig.Flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT;
        ComPtr<ID3DBlob> blob;
        CHECK(D3D12SerializeRootSignature(&sig, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
        CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(),
                                       IID_PPV_ARGS(&root)));

        D3D12_GRAPHICS_PIPELINE_STATE_DESC pso = {};
        static_assert(sizeof(pso) == 0x290 && sizeof(*inputs) == 32, "TomBox x64 PSO layout");
        pso.pRootSignature = root.Get();
        pso.VS = vs;
        pso.PS = ps;
        pso.InputLayout = {inputs, input_count};
        pso.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
        pso.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
        pso.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
        pso.RasterizerState.DepthClipEnable = TRUE;
        pso.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
        if (with_depth) {
            pso.DSVFormat = DXGI_FORMAT_D32_FLOAT;
            pso.DepthStencilState.DepthEnable = TRUE;
            pso.DepthStencilState.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ALL;
            pso.DepthStencilState.DepthFunc = D3D12_COMPARISON_FUNC_LESS;
        }
        pso.NumRenderTargets = 1;
        pso.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
        pso.SampleMask = UINT_MAX;
        pso.SampleDesc.Count = 1;
        using PipelineX = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_GRAPHICS_PIPELINE_STATE_DESC*,
                                            UINT, const void*, REFIID, void**);
        CHECK(xbox_method<PipelineX>(dev, 60)(dev, &pso, 0, nullptr, IID_PPV_ARGS(&pipeline)));

        D3D12_HEAP_PROPERTIES heap = {};
        heap.Type = D3D12_HEAP_TYPE_UPLOAD;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC buffer = {};
        buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        capacity_bytes = bytes;
        buffer.Width = bytes;
        buffer.Height = buffer.DepthOrArraySize = buffer.MipLevels = buffer.SampleDesc.Count = 1;
        buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
              D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&vertices)));
        return 0;
    }

    int bind(Frame& frame, const void* vertex_data, UINT count, UINT stride,
             const D3D12_GPU_DESCRIPTOR_HANDLE* tables = nullptr, UINT table_count = 0) {
        if (UINT64(count) * stride > capacity_bytes) {
            std::fputs("Mesh exceeds vertex buffer capacity\n", stderr);
            return 1;
        }
        auto* list = frame.list.Get();
        void* data = nullptr;
        D3D12_RANGE no_reads = {};
        CHECK(vertices->Map(0, &no_reads, &data));
        std::memcpy(data, vertex_data, count * stride);
        vertices->Unmap(0, nullptr);
        D3D12_VERTEX_BUFFER_VIEW view = {vertices->GetGPUVirtualAddress(), count * stride, stride};
        const D3D12_VIEWPORT viewport = {0, 0, float(frame.width), float(frame.height), 0, 1};
        const D3D12_RECT scissor = {0, 0, LONG(frame.width), LONG(frame.height)};
        using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
        using VertexBuffers = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const D3D12_VERTEX_BUFFER_VIEW*);
        using Targets = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_CPU_DESCRIPTOR_HANDLE*,
                                       BOOL, const D3D12_CPU_DESCRIPTOR_HANDLE*);
        xbox_method<Root>(list, 34)(list, root.Get());
        using Table = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
        for (UINT slot = 0; slot < table_count; ++slot)
            xbox_method<Table>(list, 36)(list, slot, tables[slot]);
        xbox_method<VertexBuffers>(list, 48)(list, 0, 1, &view);
        xbox_method<Targets>(list, 50)(list, 1, &frame.rtv, FALSE, frame.depth ? &frame.dsv : nullptr);
        list->SetPipelineState(pipeline.Get());
        list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list->RSSetViewports(1, &viewport);
        list->RSSetScissorRects(1, &scissor);
        return 0;
    }

    int draw(Frame& frame, const void* vertex_data, UINT count, UINT stride,
             const D3D12_GPU_DESCRIPTOR_HANDLE* tables = nullptr, UINT table_count = 0) {
        if (bind(frame, vertex_data, count, stride, tables, table_count)) return 1;
        frame.list->DrawInstanced(count, 1, 0, 0);
        return 0;
    }
};
