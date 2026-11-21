// Exercise the cache's 64-bit visibility rasterizers on a single horizontal segment.
#include "graphics.h"
#include "ps09_raster_inputs.h"
#include <cstdint>
#include <fstream>
#include <iterator>
#include <vector>

namespace {
using namespace raster_fixture;
void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* res, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {res, 0, from, to};
    list->ResourceBarrier(1, &barrier);
}

int run(const std::vector<char>& shader, UINT mode, std::ofstream& output) {
    Frame frame;
    if (frame.init(2, 2, false, 512)) return 1;
    auto dev = frame.dev.Get();
    auto list = frame.list.Get();
    D3D12_DESCRIPTOR_RANGE ranges[] = {
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 13, 0, 13}, {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 73, 0, 15},
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 72, 0, 18}, {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 71, 0, 21},
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 74, 0, 29}, {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 10, 0, 0},
        {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 15, 0, 1}, {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 2},
    };
    D3D12_ROOT_PARAMETER params[3]{};
    params[0].DescriptorTable = {5, ranges};
    params[1].DescriptorTable = {3, ranges + 5};
    params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[2].Constants = {500, 0, 5};
    D3D12_ROOT_SIGNATURE_DESC layout{3, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    ComPtr<ID3D12RootSignature> root;
    CHECK(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr));
    CHECK(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)));
    D3D12_COMPUTE_PIPELINE_STATE_DESC pso{};
    pso.pRootSignature = root.Get();
    pso.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    using Create = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*, REFIID, void**);
    CHECK(xbox_method<Create>(dev, 62)(dev, &pso, 0, nullptr, IID_PPV_ARGS(&pipeline)));

    D3D12_HEAP_PROPERTIES heap{};
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    auto buffer = frame.readback->GetDesc();
    buffer.Width = texture_offset + 1024;
    ComPtr<ID3D12Resource> upload;
    CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* data;
    CHECK(upload->Map(0, nullptr, &data));
    BYTE* bytes = static_cast<BYTE*>(data);
    std::memset(bytes, 0, size_t(buffer.Width));
    raster_fixture::initialize(bytes, mode);
    upload->Unmap(0, nullptr);

    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 33, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> views;
    CHECK(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&views)));
    using Handle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    UINT64 cpu = xbox_method<Handle>(views.Get(), 9)(views.Get());
    UINT64 gpu = xbox_method<Handle>(views.Get(), 10)(views.Get());
    UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    auto handle = [&](UINT index) -> D3D12_CPU_DESCRIPTOR_HANDLE { return {SIZE_T(cpu + index * step)}; };
    D3D12_SHADER_RESOURCE_VIEW_DESC raw{};
    raw.Format = DXGI_FORMAT_R32_TYPELESS;
    raw.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    raw.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    raw.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
    raw.Buffer.NumElements = block / 4;
    for (UINT i = 0; i < 4; i++) {
        raw.Buffer.FirstElement = i * block / 4;
        dev->CreateShaderResourceView(upload.Get(), &raw, handle(ranges[i].OffsetInDescriptorsFromTableStart));
    }
    for (UINT i = 0; i < 2; i++) {
        D3D12_CONSTANT_BUFFER_VIEW_DESC cb{upload->GetGPUVirtualAddress() + (i + 4) * block, block};
        dev->CreateConstantBufferView(&cb, handle(31 + i));
    }
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    ComPtr<ID3D12Resource> images[2];
    auto copy = [&](UINT i, bool to_image) {
        D3D12_TEXTURE_COPY_LOCATION texture{}, buf{};
        texture.pResource = images[i].Get();
        buf.pResource = to_image ? upload.Get() : frame.readback.Get();
        buf.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        buf.PlacedFootprint = {to_image ? texture_offset + 512ull * i : 0,
                               {i ? DXGI_FORMAT_R32G32_UINT : DXGI_FORMAT_R32_FLOAT, i ? 2u : 16u, i ? 2u : 1u, 1, pitch}};
        using Copy = void(WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*, UINT, UINT, UINT,
                                   const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
        xbox_method<Copy>(list, 83)(list, to_image ? &texture : &buf, 0, 0, 0, to_image ? &buf : &texture, nullptr, 0);
    };
    for (UINT i = 0; i < 2; i++) {
        D3D12_RESOURCE_DESC desc{};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        desc.Width = i ? 2 : 16;
        desc.Height = i ? 2 : 1;
        desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
        desc.Format = i ? DXGI_FORMAT_R32G32_UINT : DXGI_FORMAT_R32_FLOAT;
        desc.Flags = i ? D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS : D3D12_RESOURCE_FLAG_NONE;
        CHECK(dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                           IID_PPV_ARGS(&images[i])));
        copy(i, true);
        transition(list, images[i].Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                   i ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    }
    dev->CreateShaderResourceView(images[0].Get(), nullptr, handle(29));
    dev->CreateUnorderedAccessView(images[1].Get(), nullptr, nullptr, handle(30));
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    using Constants = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
    xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
    xbox_method<Root>(list, 33)(list, root.Get());
    xbox_method<Table>(list, 35)(list, 0, {gpu});
    xbox_method<Table>(list, 35)(list, 1, {gpu + 30 * step});
    UINT args[5]{};
    xbox_method<Constants>(list, 39)(list, 2, 5, args, 0);
    list->SetPipelineState(pipeline.Get());
    list->Dispatch(1, 1, 1);
    transition(list, images[1].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    copy(1, false);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;
    CHECK(frame.readback->Map(0, nullptr, &data));
    uint64_t result[4];
    std::memcpy(result, data, 16);
    std::memcpy(result + 2, static_cast<BYTE*>(data) + pitch, 16);
    frame.readback->Unmap(0, nullptr);
    output.write(reinterpret_cast<const char*>(result), sizeof(result));
    // Both original wave64 kernels were independently dispatched on GFX1036
    // with these inputs. Case 1 compares equal depth words on both pixels.
    const uint64_t expected[] = {mode == 2 ? 0 : winner, mode == 2 ? 0 : winner + (mode == 1), guard0, guard1};
    for (UINT i = 0; i < 4; i++) {
        if (result[i] != expected[i]) {
            std::fprintf(stderr, "raster case %u pixel %u: %016llx expected %016llx\n", mode, i, result[i], expected[i]);
            return 1;
        }
    }
    return 0;
}
} // namespace

int main(int argc, char** argv) {
    if (argc != 4 || std::strcmp(argv[1], "--visibility-raster")) return 2;
    std::ifstream file(argv[2], std::ios::binary);
    std::vector<char> shader((std::istreambuf_iterator<char>(file)), {});
    if (shader.empty()) return 1;
    std::ofstream output(argv[3], std::ios::binary);
    if (!output) return 1;
    for (UINT mode = 0; mode < 3; mode++) {
        if (run(shader, mode, output)) return 1;
    }
    output.close();
    if (!output) return 1;
    std::puts("PS09 visibility raster checked on 3 guarded 2x2 surfaces");
    return 0;
}
