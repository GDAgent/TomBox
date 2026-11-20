// Exercise the supplied FSPX container through the Xbox PSO entry point.
#define NOMINMAX
#include "graphics.h"
#include <DirectXPackedVector.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <limits>
#include <vector>

namespace {
using Pixel = std::array<float, 4>;
constexpr UINT extent = 32, pitch = extent * sizeof(Pixel), tensor_bytes = 18 * 0x3c20;
constexpr UINT guard = 0xdeadbeef;
UINT bits(float f) {
    UINT u;
    std::memcpy(&u, &f, 4);
    return u;
}

void barrier(ID3D12GraphicsCommandList* list, ID3D12Resource* res, D3D12_RESOURCE_STATES from,
             D3D12_RESOURCE_STATES to) {
    D3D12_RESOURCE_BARRIER b{};
    b.Transition = {res, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, from, to};
    list->ResourceBarrier(1, &b);
}
void copy_tex(ID3D12GraphicsCommandList* list, ID3D12Resource* tex, ID3D12Resource* buf, UINT offset,
              bool to_tex) {
    D3D12_TEXTURE_COPY_LOCATION t{}, b{};
    t.pResource = tex;
    b.pResource = buf;
    b.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    b.PlacedFootprint = {offset, {DXGI_FORMAT_R32G32B32A32_FLOAT, extent, extent, 1, pitch}};
    using Copy = void(WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*, UINT, UINT, UINT,
                               const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
    xbox_method<Copy>(list, 83)(list, to_tex ? &t : &b, 0, 0, 0, to_tex ? &b : &t, nullptr, 0);
}

int create_pipeline(ID3D12Device* dev, D3D12_SHADER_BYTECODE shader, ComPtr<ID3D12RootSignature>& root,
                    ComPtr<ID3D12PipelineState>& pso) {
    CHECK(dev->CreateRootSignature(0, shader.pShaderBytecode, shader.BytecodeLength, IID_PPV_ARGS(&root)));
    D3D12_COMPUTE_PIPELINE_STATE_DESC cs{};
    cs.pRootSignature = root.Get();
    cs.CS = shader;
    using Create = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*,
                                    REFIID, void**);
    CHECK(xbox_method<Create>(dev, 62)(dev, &cs, 0, nullptr, IID_PPV_ARGS(&pso)));
    return 0;
}

int run(D3D12_SHADER_BYTECODE shader, bool reset, UINT width, UINT height, bool mixed_motion) {
    constexpr UINT texture_bytes = extent * pitch;
    constexpr UINT raw_offset = 6 * texture_bytes;
    constexpr UINT cb_offset = raw_offset + 2048;
    constexpr UINT fill_offset = cb_offset + 256;
    Frame frame;
    if (frame.init(1, 1, false, tensor_bytes + texture_bytes + 512))
        return 1;
    auto dev = frame.dev.Get();
    auto list = frame.list.Get();
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pso;
    if (create_pipeline(dev, shader, root, pso)) return 1;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_UPLOAD;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    auto desc = frame.readback->GetDesc();
    desc.Width = fill_offset + tensor_bytes;
    ComPtr<ID3D12Resource> upload, tensor;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* data;
    CHECK(upload->Map(0, nullptr, &data));
    auto* bytes = static_cast<unsigned char*>(data);
    std::memset(bytes, 0, size_t(desc.Width));
    const Pixel color = {1, 3, 7, 1};
    for (UINT slot = 0; slot < 6; ++slot) {
        for (UINT i = 0; i < extent * extent; ++i) {
            Pixel texel{};
            if (slot == 0)
                texel = {0.25f + (i % extent) / 128.0f, 0.375f + (i / extent) / 128.0f, 0.5f, 1};
            if (slot == 1 && mixed_motion && (i % 2 == 0))
                texel[0] = std::numeric_limits<float>::quiet_NaN();
            if (slot == 2)
                texel[0] = 1;
            if (slot == 3)
                texel = color;
            if (slot == 4)
                texel = {0.25f, 0.5f, 0.75f, 1};
            if (slot == 5)
                texel.fill(-55.0f);
            std::memcpy(bytes + slot * texture_bytes + i * sizeof(Pixel), texel.data(), sizeof(texel));
        }
    }
    const float exposure[] = {1, 1};
    std::memcpy(bytes + raw_offset, exposure, sizeof(exposure));
    // Each 2x2 quadrant has its own four bias channels. On reset, also
    // select current luminance for output channel zero with a half weight of 1/4.
    if (reset)
        for (UINT record : {0u, 17u, 40u, 57u}) {
            const UINT weight = 0x3400;
            std::memcpy(bytes + raw_offset + 256 + record * 16, &weight, 4);
        }
    for (UINT i = 0; i < 16; ++i) {
        float bias = (int(i) - 7) / 64.0f;
        std::memcpy(bytes + raw_offset + 256 + 64 * 16 + i * 4, &bias, 4);
    }
    UINT constants[24] = {bits(float(width)),
                          bits(float(height)),
                          bits(1),
                          bits(1),
                          bits(1),
                          bits(1),
                          0,
                          0,
                          bits(1),
                          bits(1),
                          bits(float(extent)),
                          bits(float(extent)),
                          0,
                          0,
                          0,
                          0,
                          width,
                          height,
                          UINT(reset),
                          extent,
                          extent,
                          bits(150),
                          bits(150),
                          0};
    std::memcpy(bytes + cb_offset, constants, sizeof(constants));
    std::fill_n(reinterpret_cast<UINT*>(bytes + fill_offset), tensor_bytes / 4, guard);
    upload->Unmap(0, nullptr);
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    desc.Width = tensor_bytes;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, nullptr,
                                       IID_PPV_ARGS(&tensor)));
    list->CopyBufferRegion(tensor.Get(), 0, upload.Get(), fill_offset, tensor_bytes);
    barrier(list, tensor.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 34,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> views;
    CHECK(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&views)));
    using Handle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    const auto base = xbox_method<Handle>(views.Get(), 9)(views.Get());
    const auto gpu = xbox_method<Handle>(views.Get(), 10)(views.Get());
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    auto cpu = [&](UINT slot) { return D3D12_CPU_DESCRIPTOR_HANDLE{SIZE_T(base + slot * step)}; };
    D3D12_UNORDERED_ACCESS_VIEW_DESC u{};
    u.Format = DXGI_FORMAT_R32_TYPELESS;
    u.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    u.Buffer.NumElements = tensor_bytes / 4;
    u.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
    dev->CreateUnorderedAccessView(tensor.Get(), nullptr, &u, cpu(11));
    for (UINT i = 0; i < 2; ++i) {
        D3D12_SHADER_RESOURCE_VIEW_DESC s{};
        s.Format = DXGI_FORMAT_R32_TYPELESS;
        s.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        s.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        s.Buffer.FirstElement = (raw_offset + 256 * i) / 4;
        s.Buffer.NumElements = i ? 68 * 4 : 2;
        s.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        dev->CreateShaderResourceView(upload.Get(), &s, cpu(13 + 17 + i));
    }
    std::array<ComPtr<ID3D12Resource>, 6> textures;
    desc = {};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = extent;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
    for (UINT i = 0; i < 6; ++i) {
        bool output = i == 5;
        if (output)
            desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                           nullptr, IID_PPV_ARGS(&textures[i])));
        auto t = textures[i].Get();
        copy_tex(list, t, upload.Get(), i * texture_bytes, true);
        barrier(list, t, D3D12_RESOURCE_STATE_COPY_DEST,
                output ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        if (output)
            dev->CreateUnorderedAccessView(t, nullptr, nullptr, cpu(3));
        else
            dev->CreateShaderResourceView(t, nullptr, cpu(13 + i));
    }
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    using Address = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
    xbox_method<Root>(list, 33)(list, root.Get());
    xbox_method<Table>(list, 35)(list, 0, {gpu});
    xbox_method<Table>(list, 35)(list, 1, {gpu + 13 * step});
    xbox_method<Address>(list, 41)(list, 3, upload->GetGPUVirtualAddress() + cb_offset);
    list->SetPipelineState(pso.Get());
    list->Dispatch(2, 2, 1);
    barrier(list, textures[5].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    barrier(list, tensor.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    copy_tex(list, textures[5].Get(), frame.readback.Get(), 0, false);
    list->CopyBufferRegion(frame.readback.Get(), texture_bytes, tensor.Get(), 0, tensor_bytes);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu())
        return 1;
    CHECK(frame.readback->Map(0, nullptr, &data));
    std::vector<UINT> expected(tensor_bytes / 4, guard);
    const float log_scale = 0.08137548f;
    Pixel current{};
    for (UINT c = 0; c < 4; ++c)
        current[c] = log_scale * std::log2(1 + color[c % 3]);
    float luma = 0.2126f * current[0] + 0.7152f * current[1] + 0.0722f * current[2];
    int convolution = reset ? int(std::nearbyint(16 * (2 * luma - 1))) : 0;
    unsigned mismatches = 0;
    for (UINT y = 0; y < extent; ++y)
        for (UINT x = 0; x < extent; ++x) {
            const bool active = x < width && y < height;
            if (active) {
                UINT quadrant = (x % 2) * 2 + y % 2;
                UINT packed = 0;
                for (UINT c = 0; c < 4; ++c)
                    packed |= UINT(uint8_t(int(quadrant * 4 + c) - 7 + (c == 0 ? convolution : 0))) << (8 * c);
                expected[(y / 2 + 1) * (0x3c20 / 4) + (x / 2 + 1) * 4 + quadrant] = packed;
            }
            const auto& pixel = static_cast<Pixel*>(data)[y * extent + x];
            for (UINT c = 0; c < 4; ++c) {
                const Pixel history = {0.25f + x / 128.0f, 0.375f + y / 128.0f, 0.5f, 0.25f + x / 128.0f};
                const float want = !active                      ? -55.0f
                                   : reset                      ? current[c]
                                   : mixed_motion && x % 2 == 0 ? 0.0f
                                                                : 0.9f * history[c] + 0.1f * current[c];
                if (!(std::abs(pixel[c] - want) < 0.0001f) && mismatches++ < 4)
                    std::fprintf(stderr, "u3 (%u,%u).%u expected %g got %g\n", x, y, c, want, pixel[c]);
            }
        }
    const auto* got = reinterpret_cast<UINT*>(static_cast<unsigned char*>(data) + texture_bytes);
    for (UINT i = 0; i < expected.size(); ++i)
        if (got[i] != expected[i] && mismatches++ < 8)
            std::fprintf(stderr, "u11 byte %#x expected %#x got %#x\n", i * 4, expected[i], got[i]);
    frame.readback->Unmap(0, nullptr);
    return mismatches != 0;
}
// Independent five-tap reference for the packed-half RCAS filter. The kernel
// follows the filter with inverse log encoding and exposure correction.
using namespace DirectX::PackedVector;
float half_round(float x) { return XMConvertHalfToFloat(XMConvertFloatToHalf(x)); }
float approximate_reciprocal(float x) {
    const float estimate = XMConvertHalfToFloat(UINT16(0x778d - XMConvertFloatToHalf(x)));
    return half_round(estimate * half_round(2 - estimate * x));
}

Pixel input_color(int x, int y) {
    if (x < 0 || y < 0 || x >= int(extent) || y >= int(extent)) return {};
    return {(4 + (x * 3 + y) % 11) / 32.f, (2 + (x + y * 5) % 7) / 32.f,
            (3 + (x * 2 + y * 3) % 9) / 32.f, 1};
}

Pixel sharpen_reference(int x, int y, float strength, float exposure_scale) {
    const Pixel center = input_color(x, y);
    const Pixel ring[] = {input_color(x, y - 1), input_color(x - 1, y),
                          input_color(x + 1, y), input_color(x, y + 1)};
    auto luma = [](Pixel p) { return p[1] + 0.5f * (p[0] + p[2]); };
    float ring_min_luma = std::numeric_limits<float>::infinity(), max_luma = luma(center), sum_luma = 0;
    Pixel sum{};
    float lobe = -0.1875f;
    for (auto tap : ring) {
        float l = luma(tap);
        ring_min_luma = std::min(ring_min_luma, l);
        max_luma = std::max(max_luma, l);
        sum_luma += l;
        for (UINT c = 0; c < 3; ++c) sum[c] += tap[c];
    }
    // The Scarlett pass scales the dark limiter by center/ring-min luminance.
    const float dark_scale = std::min(1.f, half_round(luma(center) * half_round(1 / ring_min_luma)));
    for (UINT c = 0; c < 3; ++c) {
        const float low = std::min({ring[0][c], ring[1][c], ring[2][c], ring[3][c]});
        const float high = std::max({ring[0][c], ring[1][c], ring[2][c], ring[3][c]});
        const float hit_min = half_round(half_round(low * half_round(1 / (4 * high))) * dark_scale);
        const float hit_max = half_round((1 - high) * half_round(1 / (4 * low - 4)));
        lobe = std::max({lobe, -hit_min, hit_max});
    }
    const float noise = std::clamp(half_round(std::abs(0.25f * sum_luma - luma(center)) *
                                              approximate_reciprocal(max_luma - std::min(ring_min_luma, luma(center)))), 0.f, 1.f);
    lobe = half_round(half_round(std::min(lobe, 0.f) * strength) * half_round(1 - 0.5f * noise));
    const float norm = approximate_reciprocal(half_round(1 + 4 * lobe));
    Pixel result{};
    for (UINT c = 0; c < 3; ++c) {
        float filtered = half_round(half_round(sum[c] * lobe + center[c]) * norm);
        result[c] = std::max(0.f, std::exp2(12.288704f * filtered) - 1) * (1.f / 150) * exposure_scale;
    }
    result[3] = result[0];
    return result;
}

int run_sharpen(D3D12_SHADER_BYTECODE shader, float strength, UINT groups_x, UINT groups_y) {
    Frame frame;
    constexpr UINT tex_bytes = extent * pitch, raw_offset = 2 * tex_bytes, cb_offset = raw_offset + 256;
    if (frame.init(1, 1, false, tex_bytes)) return 1;
    auto dev = frame.dev.Get();
    auto list = frame.list.Get();
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pso;
    if (create_pipeline(dev, shader, root, pso)) return 1;
    auto buffer = frame.readback->GetDesc();
    buffer.Width = cb_offset + 256;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_UPLOAD;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> upload;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_GENERIC_READ,
                                       nullptr, IID_PPV_ARGS(&upload)));
    void* data;
    CHECK(upload->Map(0, nullptr, &data));
    auto bytes = static_cast<BYTE*>(data);
    std::memset(bytes, 0, size_t(buffer.Width));
    for (UINT y = 0; y < extent; ++y) for (UINT x = 0; x < extent; ++x) {
        const Pixel input = input_color(x, y), sentinel = {-55, -55, -55, -55};
        const UINT pos = y * pitch + x * sizeof(Pixel);
        std::memcpy(bytes + pos, input.data(), sizeof(input));
        std::memcpy(bytes + tex_bytes + pos, sentinel.data(), sizeof(sentinel));
    }
    const float exposure = 2, pre_exposure = strength ? 3.f : 1.f;
    const UINT constants[] = {0, XMConvertFloatToHalf(strength), 0, 0, bits(pre_exposure)};
    std::memcpy(bytes + raw_offset, &exposure, 4);
    std::memcpy(bytes + cb_offset, constants, sizeof(constants));
    upload->Unmap(0, nullptr);
    D3D12_DESCRIPTOR_HEAP_DESC heap{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 4,
                                   D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
    ComPtr<ID3D12DescriptorHeap> views;
    CHECK(dev->CreateDescriptorHeap(&heap, IID_PPV_ARGS(&views)));
    using Handle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    const auto base = xbox_method<Handle>(views.Get(), 9)(views.Get());
    const auto gpu = xbox_method<Handle>(views.Get(), 10)(views.Get());
    const UINT step = dev->GetDescriptorHandleIncrementSize(heap.Type);
    auto cpu = [&](UINT slot) { return D3D12_CPU_DESCRIPTOR_HANDLE{SIZE_T(base + slot * step)}; };
    D3D12_SHADER_RESOURCE_VIEW_DESC raw{};
    raw.Format = DXGI_FORMAT_R32_TYPELESS;
    raw.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    raw.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    raw.Buffer.FirstElement = raw_offset / 4;
    raw.Buffer.NumElements = 1;
    raw.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
    dev->CreateShaderResourceView(upload.Get(), &raw, cpu(2)); // t17 after t6
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = extent;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    ComPtr<ID3D12Resource> input, output;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                       nullptr, IID_PPV_ARGS(&input)));
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    CHECK(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                       nullptr, IID_PPV_ARGS(&output)));
    copy_tex(list, input.Get(), upload.Get(), 0, true);
    copy_tex(list, output.Get(), upload.Get(), tex_bytes, true);
    barrier(list, input.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    barrier(list, output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    D3D12_SHADER_RESOURCE_VIEW_DESC unused{};
    unused.Format = desc.Format;
    unused.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
    unused.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    unused.Texture2D.MipLevels = 1;
    dev->CreateShaderResourceView(nullptr, &unused, cpu(1)); // unused t6 must never supply t18
    dev->CreateShaderResourceView(input.Get(), nullptr, cpu(3)); // t18
    dev->CreateUnorderedAccessView(output.Get(), nullptr, nullptr, cpu(0)); // u11
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    using Address = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
    xbox_method<Heaps>(list, 32)(list, 1, views.GetAddressOf());
    xbox_method<Root>(list, 33)(list, root.Get());
    xbox_method<Table>(list, 35)(list, 0, {gpu});
    xbox_method<Table>(list, 35)(list, 1, {gpu + step});
    xbox_method<Address>(list, 41)(list, 2, upload->GetGPUVirtualAddress() + cb_offset);
    list->SetPipelineState(pso.Get());
    list->Dispatch(groups_x, groups_y, 1);
    barrier(list, output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    copy_tex(list, output.Get(), frame.readback.Get(), 0, false);
    CHECK(list->Close());
    ID3D12CommandList* lists[] = {list};
    frame.queue->ExecuteCommandLists(1, lists);
    if (frame.wait_for_gpu()) return 1;
    CHECK(frame.readback->Map(0, nullptr, &data));
    unsigned mismatches = 0;
    for (UINT y = 0; y < extent; ++y) for (UINT x = 0; x < extent; ++x) {
        const bool active = x < 16 * groups_x && y < 16 * groups_y;
        const auto want = active ? sharpen_reference(x, y, strength, pre_exposure / exposure)
                                 : Pixel{-55, -55, -55, -55};
        const auto& got = static_cast<const Pixel*>(data)[y * extent + x];
        for (UINT c = 0; c < 4; ++c) {
            if (!(std::abs(got[c] - want[c]) <= (active ? 0.00001f + 0.002f * want[c] : 0.f))) {
                if (mismatches++ < 6) std::fprintf(stderr, "strength %g (%u,%u).%u expected %g got %g\n",
                                                   strength, x, y, c, want[c], got[c]);
            }
        }
    }
    frame.readback->Unmap(0, nullptr);
    if (mismatches) std::fprintf(stderr, "%u channel mismatches\n", mismatches);
    return mismatches != 0;
}
} // namespace

int wmain(int argc, wchar_t** argv) {
    const bool sharpen = argc == 2 && !std::wcscmp(argv[1], L"--sharpen");
    if (argc != 1 && !sharpen) return 2;
    const auto path = _wgetenv(L"TOMBOX_FSR_LIBRARY");
    if (!path)
        return 2;
    std::ifstream file(std::filesystem::path(path), std::ios::binary);
    std::vector<char> dll((std::istreambuf_iterator<char>(file)), {});
    for (size_t i = 0; i + 32 <= dll.size(); ++i) {
        if (std::memcmp(dll.data() + i, "DXBC", 4))
            continue;
        const char* p = dll.data() + i;
        UINT size, count;
        std::memcpy(&size, p + 24, 4);
        std::memcpy(&count, p + 28, 4);
        if (size < 32 || size > dll.size() - i || count > (size - 32) / 4)
            continue;
        uint64_t h = 0xcbf29ce484222325ULL;
        for (UINT j = 0; j < size; ++j)
            h = (h ^ uint8_t(p[j])) * 0x100000001b3ULL;
        if (h != (sharpen ? 0x627f8369a1fe920dULL : 0xa2c897d4756321ddULL))
            continue;
        // CreateRootSignature accepts the complete container carrying the RTS0 chunk.
        D3D12_SHADER_BYTECODE shader{p, size};
        if (sharpen) {
            if (run_sharpen(shader, 0.f, 2, 1) || run_sharpen(shader, 1.f, 1, 2)) return 1;
            std::puts("Retail sharpening passed two strengths, exposure scaling and guard-texel readback.");
            return 0;
        }
        if (run(shader, true, 32, 32, false) || run(shader, false, 23, 19, true) ||
            run(shader, false, 30, 28, false))
            return 1;
        std::puts(
            "Retail FSR4 prepass passed reset, mixed NaN/finite motion and history texture/tensor readbacks.");
        return 0;
    }
    std::fputs("Missing requested FSR4 kernel or root signature\n", stderr);
    return 1;
}
