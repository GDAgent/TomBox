// Draw the cache's tagged-depth pass with deterministic visibility/material data.
#include "graphics.h"
#include "triangle_shaders.h"
#include <array>
#include <fstream>
#include <iterator>
#include <vector>

int main(int argc, char** argv) {
    if (argc != 4 || std::strcmp(argv[1], "--tagged-depth"))
        return 2;
    std::ifstream file(argv[2], std::ios::binary);
    std::vector<char> ps((std::istreambuf_iterator<char>(file)), {});
    if (ps.empty())
        return 1;
    constexpr UINT size = 16, pitch = 256, raw_start = size * pitch;
    constexpr UINT records = 40, stride = 336, field = 140;
    Frame frame;
    if (frame.init(size, size, true) || frame.begin({0, 0, 0, 1}))
        return 1;
    auto dev = frame.dev.Get();
    auto list = frame.list.Get();
    D3D12_DESCRIPTOR_RANGE ranges[] = {
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 0, 0, 21},
        {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 1, 0, 0},
    };
    D3D12_ROOT_PARAMETER params[3]{};
    for (UINT i = 0; i < 3; ++i)
        params[i].ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
    for (UINT i = 0; i < 2; ++i) {
        params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        params[i].DescriptorTable = {1, &ranges[i]};
    }
    params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[2].Constants = {0, 0, 1};
    const D3D12_INPUT_ELEMENT_DESC input{"POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT};
    const float vertices[][4] = {{-1, -1, 0, 1}, {-1, 3, 0, 1}, {3, -1, 0, 1}};
    Mesh mesh;
    if (mesh.init(dev, {triangle_vs, sizeof(triangle_vs)}, {ps.data(), ps.size()}, &input, 1, sizeof(vertices),
                  params, 3, true))
        return 1;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_UPLOAD;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    auto buffer = frame.readback->GetDesc();
    buffer.Width = raw_start + records * stride;
    ComPtr<ID3D12Resource> upload, image;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* data;
    CHECK(upload->Map(0, nullptr, &data));
    BYTE* bytes = static_cast<BYTE*>(data);
    std::memset(bytes, 0xee, size_t(buffer.Width));
    for (UINT rec = 0; rec < records; ++rec) {
        UINT material = 0xaa000055 | (((rec % 3) ? 83u : 31u) << 16);
        std::memcpy(bytes + raw_start + stride * rec + field, &material, 4);
    }
    for (UINT y = 0; y < size; ++y)
        for (UINT x = 0; x < size; ++x) {
            UINT rec = (x + 3 * y) % 8;
            UINT high = ((0x10000 + (x * 359 + y * 1087) * 293) << 8) | (y % 2);
            UINT low = (rec << 27) | 1;
            switch ((x + 2 * y) % 5) {
            case 0:
                low = high = 0;
                break;
            case 1:
                low = 0;
                break;
            case 2:
                low &= ~1u;
                break;
            }
            UINT pair[] = {low, high};
            std::memcpy(bytes + y * pitch + x * sizeof(pair), pair, sizeof(pair));
        }
    upload->Unmap(0, nullptr);
    D3D12_RESOURCE_DESC tex{};
    tex.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    tex.Width = tex.Height = size;
    tex.DepthOrArraySize = tex.MipLevels = tex.SampleDesc.Count = 1;
    tex.Format = DXGI_FORMAT_R32G32_UINT;
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &tex, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&image)));
    auto copy = [&](ID3D12Resource* t, ID3D12Resource* b, DXGI_FORMAT format, bool to_texture) {
        D3D12_TEXTURE_COPY_LOCATION texture{}, buf{};
        texture.pResource = t;
        buf.pResource = b;
        buf.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        buf.PlacedFootprint = {0, {format, size, size, 1, pitch}};
        using Copy = void(WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*, UINT, UINT,
                                   UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
        xbox_method<Copy>(list, 83)(list, to_texture ? &texture : &buf, 0, 0, 0, to_texture ? &buf : &texture,
                                    nullptr, 0);
    };
    copy(image.Get(), upload.Get(), tex.Format, true);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {image.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST,
                          D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE};
    list->ResourceBarrier(1, &barrier);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 23,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> views;
    CHECK(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&views)));
    using Handle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    UINT64 base = xbox_method<Handle>(views.Get(), 9)(views.Get());
    UINT64 gpu = xbox_method<Handle>(views.Get(), 10)(views.Get());
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    D3D12_SHADER_RESOURCE_VIEW_DESC raw{};
    raw.Format = DXGI_FORMAT_R32_TYPELESS;
    raw.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    raw.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    raw.Buffer.FirstElement = raw_start / 4;
    raw.Buffer.NumElements = records * stride / 4;
    raw.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
    dev->CreateShaderResourceView(upload.Get(), &raw, {SIZE_T(base + 21 * step)});
    dev->CreateShaderResourceView(image.Get(), nullptr, {SIZE_T(base + 22 * step)});
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
    D3D12_GPU_DESCRIPTOR_HANDLE tabs[] = {{gpu}, {gpu + 22 * step}};
    if (mesh.bind(frame, vertices, 3, sizeof(vertices[0]), tabs, 2))
        return 1;
    using Constant = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, UINT);
    xbox_method<Constant>(list, 38)(list, 2, 83, 0);
    list->DrawInstanced(3, 1, 0, 0);
    barrier.Transition = {frame.depth.Get(), 0, D3D12_RESOURCE_STATE_DEPTH_WRITE,
                          D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    copy(frame.depth.Get(), frame.readback.Get(), DXGI_FORMAT_R32_FLOAT, false);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu())
        return 1;
    CHECK(frame.readback->Map(0, nullptr, &data));
    std::ofstream out(argv[3], std::ios::binary);
    for (UINT y = 0; y < size; ++y)
        out.write(static_cast<char*>(data) + y * pitch, size * 4);
    out.close();
    frame.readback->Unmap(0, nullptr);
    return !out;
}
