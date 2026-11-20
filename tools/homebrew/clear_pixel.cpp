// Headless synthetic title. Xbox slot numbers follow TomBox's device_wrap,
// cmdlist_wrap and heap_wrap. All other calls use the public Windows SDK.
#include <windows.h>
#include <d3d12.h>
#include <wrl/client.h>
#include <cmath>
#include <cstdio>
#include <cstring>
#include "triangle_shaders.h"
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

static int check_root_signature() {
    D3D12_DESCRIPTOR_RANGE range = {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 2, 3, 0, 0};
    D3D12_ROOT_PARAMETER param = {};
    param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    param.DescriptorTable = {1, &range};
    param.ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
    D3D12_ROOT_SIGNATURE_DESC desc = {1, &param};
    ComPtr<ID3DBlob> blob;
    CHECK(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    ComPtr<ID3D12RootSignatureDeserializer> reader;
    CHECK(D3D12CreateRootSignatureDeserializer(blob->GetBufferPointer(), blob->GetBufferSize(),
                                               IID_PPV_ARGS(&reader)));
    const auto* decoded = reader->GetRootSignatureDesc();
    if (!decoded || decoded->NumParameters != 1 ||
        decoded->pParameters[0].ParameterType != D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE)
        return 1;
    const auto& table = decoded->pParameters[0].DescriptorTable;
    if (table.NumDescriptorRanges != 1 || table.pDescriptorRanges[0].BaseShaderRegister != 3 ||
        table.pDescriptorRanges[0].NumDescriptors != 2 ||
        table.pDescriptorRanges[0].RangeType != D3D12_DESCRIPTOR_RANGE_TYPE_SRV)
        return 1;
    std::puts("Root signature round trip: two SRVs starting at t3");
    return 0;
}

// Keep GPU objects alive until the queue completes.
struct Triangle {
    ComPtr<ID3D12Resource> vertices;
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pipeline;

    int draw(ID3D12Device* dev, ID3D12GraphicsCommandList* list,
             D3D12_CPU_DESCRIPTOR_HANDLE rtv, UINT extent) {
        D3D12_ROOT_SIGNATURE_DESC sig = {};
        sig.Flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT;
        ComPtr<ID3DBlob> blob;
        CHECK(D3D12SerializeRootSignature(&sig, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
        CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(),
                                       IID_PPV_ARGS(&root)));

        const D3D12_INPUT_ELEMENT_DESC position = {
            "POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 0, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0
        };
        D3D12_GRAPHICS_PIPELINE_STATE_DESC pso = {};
        static_assert(sizeof(pso) == 0x290 && sizeof(position) == 32, "TomBox x64 PSO layout");
        pso.pRootSignature = root.Get();
        pso.VS = {triangle_vs, sizeof(triangle_vs)};
        pso.PS = {triangle_ps, sizeof(triangle_ps)};
        pso.InputLayout = {&position, 1};
        pso.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
        pso.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
        pso.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
        pso.RasterizerState.DepthClipEnable = TRUE;
        pso.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
        pso.NumRenderTargets = 1;
        pso.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
        pso.SampleMask = UINT_MAX;
        pso.SampleDesc.Count = 1;
        using PipelineX = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_GRAPHICS_PIPELINE_STATE_DESC*,
                                            UINT, const void*, REFIID, void**);
        CHECK(xbox_method<PipelineX>(dev, 60)(dev, &pso, 0, nullptr, IID_PPV_ARGS(&pipeline)));

        const float positions[][4] = {{-.75f, -.75f, 0, 1}, {0, .75f, 0, 1}, {.75f, -.75f, 0, 1}};
        D3D12_HEAP_PROPERTIES heap = {};
        heap.Type = D3D12_HEAP_TYPE_UPLOAD;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC buffer = {};
        buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        buffer.Width = sizeof(positions);
        buffer.Height = buffer.DepthOrArraySize = buffer.MipLevels = buffer.SampleDesc.Count = 1;
        buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
              D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&vertices)));
        void* data = nullptr;
        D3D12_RANGE no_reads = {};
        CHECK(vertices->Map(0, &no_reads, &data));
        std::memcpy(data, positions, sizeof(positions));
        vertices->Unmap(0, nullptr);
        D3D12_VERTEX_BUFFER_VIEW view = {vertices->GetGPUVirtualAddress(), sizeof(positions), sizeof(positions[0])};
        const D3D12_VIEWPORT viewport = {0, 0, float(extent), float(extent), 0, 1};
        const D3D12_RECT scissor = {0, 0, LONG(extent), LONG(extent)};
        using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
        using VertexBuffers = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const D3D12_VERTEX_BUFFER_VIEW*);
        using Targets = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_CPU_DESCRIPTOR_HANDLE*,
                                       BOOL, const D3D12_CPU_DESCRIPTOR_HANDLE*);
        xbox_method<Root>(list, 34)(list, root.Get());
        xbox_method<VertexBuffers>(list, 48)(list, 0, 1, &view);
        xbox_method<Targets>(list, 50)(list, 1, &rtv, FALSE, nullptr);
        list->SetPipelineState(pipeline.Get());
        list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list->RSSetViewports(1, &viewport);
        list->RSSetScissorRects(1, &scissor);
        list->DrawInstanced(3, 1, 0, 0);
        return 0;
    }
};

int main(int argc, char** argv) {
    const bool draw_triangle = argc == 2 && std::strcmp(argv[1], "--triangle") == 0;
    if (argc != 1 && !draw_triangle) {
        std::fputs("usage: clear_pixel.exe [--triangle]\n", stderr);
        return 1;
    }
    const UINT extent = draw_triangle ? 16 : 1;
    const UINT pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
    if (check_root_signature()) {
        std::fputs("Root signature round trip failed\n", stderr);
        return 1;
    }
    ComPtr<ID3D12Device> dev;
    CHECK(D3D12XboxCreateDevice(nullptr, nullptr, IID_PPV_ARGS(&dev)));
    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC qdesc = {};
    using QueueX = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMMAND_QUEUE_DESC*, REFIID, void**);
    CHECK(xbox_method<QueueX>(dev.Get(), 48)(dev.Get(), &qdesc, IID_PPV_ARGS(&queue)));
    ComPtr<ID3D12CommandAllocator> alloc;
    CHECK(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)));
    ComPtr<ID3D12GraphicsCommandList> list;
    const UINT params[4] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    using ListX = HRESULT (WINAPI*)(ID3D12Device*, const UINT*, ID3D12CommandAllocator*,
                                    ID3D12PipelineState*, REFIID, void**);
    CHECK(xbox_method<ListX>(dev.Get(), 73)(dev.Get(), params, alloc.Get(), nullptr, IID_PPV_ARGS(&list)));

    D3D12_HEAP_PROPERTIES heap = {};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc = {};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = extent;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> target;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
          D3D12_RESOURCE_STATE_RENDER_TARGET, nullptr, IID_PPV_ARGS(&target)));

    ComPtr<ID3D12DescriptorHeap> views;
    D3D12_DESCRIPTOR_HEAP_DESC hdesc = {D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1};
    CHECK(dev->CreateDescriptorHeap(&hdesc, IID_PPV_ARGS(&views)));
    // TomBox returns this handle directly in RAX (Xbox ABI).
    using CpuHandle = SIZE_T (WINAPI*)(ID3D12DescriptorHeap*);
    D3D12_CPU_DESCRIPTOR_HANDLE rtv = {xbox_method<CpuHandle>(views.Get(), 9)(views.Get())};
    dev->CreateRenderTargetView(target.Get(), nullptr, rtv);

    heap.Type = D3D12_HEAP_TYPE_READBACK;
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = extent * pitch;
    desc.Height = 1;
    desc.Format = DXGI_FORMAT_UNKNOWN;
    desc.Flags = D3D12_RESOURCE_FLAG_NONE;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> readback;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
          D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&readback)));

    const float magenta[4] = {1, 0, 1, 1};
    using ClearX = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                                  const float*, UINT, const D3D12_RECT*);
    xbox_method<ClearX>(list.Get(), 52)(list.Get(), rtv, magenta, 0, nullptr);
    Triangle triangle;
    if (draw_triangle && triangle.draw(dev.Get(), list.Get(), rtv, extent)) return 1;
    D3D12_RESOURCE_BARRIER barrier = {};
    barrier.Transition = {target.Get(), 0, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    D3D12_TEXTURE_COPY_LOCATION dst = {}, src = {};
    dst.pResource = readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint.Footprint = {DXGI_FORMAT_R8G8B8A8_UNORM, extent, extent, 1, pitch};
    src.pResource = target.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    using CopyX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                 UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
    xbox_method<CopyX>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
    CHECK(list->Close());
    ComPtr<ID3D12Fence> fence;
    CHECK(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)));
    ID3D12CommandList* work[] = {list.Get()};
    queue->ExecuteCommandLists(1, work);
    CHECK(queue->Signal(fence.Get(), 1));
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) return 1;
    HRESULT armed = fence->SetEventOnCompletion(1, done);
    DWORD wait = SUCCEEDED(armed) ? WaitForSingleObject(done, 10000) : WAIT_FAILED;
    CloseHandle(done);
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1) {
        std::fprintf(stderr, "GPU fence failed (wait=%lu, event HRESULT=0x%08lx)\n", wait, armed);
        return 1;
    }
    D3D12_RANGE range = {0, (extent - 1) * pitch + extent * 4};
    void* mapped = nullptr;
    CHECK(readback->Map(0, &range, &mapped));
    const auto* bytes = static_cast<const unsigned char*>(mapped);
    auto pixel_matches = [&](UINT x, UINT y, const unsigned char (&expected)[4]) {
        const auto* pixel = bytes + y * pitch + 4 * x;
        if (draw_triangle) std::printf("(%u,%u) ", x, y);
        std::printf("RGBA readback: %u %u %u %u\n", pixel[0], pixel[1], pixel[2], pixel[3]);
        return std::memcmp(pixel, expected, 4) == 0;
    };
    const unsigned char background[] = {255, 0, 255, 255};
    const unsigned char green[] = {0, 255, 0, 255};
    bool pass = pixel_matches(0, 0, background);
    if (draw_triangle) {
        pass = pixel_matches(extent / 2, extent / 2, green) && pass;
        // In this 16x16 viewport the vertices land at (2,14), (8,2), (14,14).
        // No pixel center lies on an edge, so coverage is independent of tie-breaking.
        unsigned mismatches = 0;
        for (UINT y = 0; y < extent; ++y) {
            for (UINT x = 0; x < extent; ++x) {
                float sx = x + 0.5f, sy = y + 0.5f;
                bool inside = sy < 14 && 2 * std::fabs(sx - 8) < sy - 2;
                if (std::memcmp(bytes + y * pitch + x * 4, inside ? green : background, 4) != 0)
                    ++mismatches;
            }
        }
        std::printf("Triangle coverage: %u/%u pixels match\n", extent * extent - mismatches, extent * extent);
        pass = pass && mismatches == 0;
    }
    D3D12_RANGE no_writes = {};
    readback->Unmap(0, &no_writes);
    return pass ? 0 : 1;
}
