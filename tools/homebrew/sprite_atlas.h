#pragma once
#include "graphics.h"
#include <array>
#include <memory>

extern "C" __declspec(dllimport) void* WINAPI XMemAlloc(size_t, UINT64);
extern "C" __declspec(dllimport) void WINAPI XMemFree(void*, UINT64);

// Four hand-authored 4x4 RGBA8 sprites in one row. The asymmetric highlights
// make channel swaps, rotations and reversed UVs visible in the readback.
using Color = std::array<unsigned char, 4>;
enum Sprite { Floor, Wall, Coin, Player };
using Pattern = std::array<const char*, 4>;
constexpr Pattern patterns[] = {
    {"0001", "0000", "0000", "1000"},
    {"2223", "3333", "2322", "3333"},
    {"0450", "4657", "7557", "0750"},
    {"0880", "8998", "8A88", "0880"},
};
constexpr Color palette[] = {
    {37,51,70,255}, {49,67,87,255}, {152,162,187,255}, {85,97,118,255},
    {255,239,161,255}, {255,196,52,255}, {255,255,220,255}, {185,100,30,255},
    {56,209,224,255}, {13,64,79,255}, {162,250,248,255},
};

// The final coin turns silver. The alternate sprite exists only in the upload
// buffer until the first pickup copies it over the gold coin in the GPU atlas.
constexpr Color silver[] = {
    {220,239,255,255}, {159,200,235,255}, {250,253,255,255}, {75,108,147,255},
};

inline Color sprite_texel(Sprite sprite, UINT x, UINT y, bool final_coin = false) {
    char c = patterns[sprite][y][x];
    if (final_coin && sprite == Coin && c >= '4' && c <= '7') return silver[c - '4'];
    return palette[c == 'A' ? 10 : c - '0'];
}

struct SpriteAtlas {
    static constexpr UINT width = 16, height = 4;
    static constexpr UINT64 placement_attributes = 0x30804000; // XMem texture memory, 64 KiB aligned.
    struct FreePlacement {
        void operator()(void* p) const { XMemFree(p, placement_attributes); }
    };
    std::unique_ptr<void, FreePlacement> placement;
    ComPtr<ID3D12Resource> texture, upload;
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint{};
    static constexpr UINT cpu_width = 64, cpu_height = 32;
    bool cpu_tiled = false;
    bool final_coin_uploaded = false;
    ComPtr<ID3D12DescriptorHeap> views, samplers;
    D3D12_GPU_DESCRIPTOR_HANDLE tables[2]{};

    void transition(Frame& frame, D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
        D3D12_RESOURCE_BARRIER barrier = {};
        barrier.Transition = {texture.Get(), 0, before, after};
        frame.list->ResourceBarrier(1, &barrier);
    }

    void copy_columns(Frame& frame, UINT first, UINT count, UINT dest) {
        D3D12_TEXTURE_COPY_LOCATION src = {}, dst = {};
        src.pResource = upload.Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint = footprint;
        dst.pResource = texture.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        D3D12_BOX box = {first, 0, 0, first + count, height, 1};
        using CopyX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                     UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
        xbox_method<CopyX>(frame.list.Get(), 83)(frame.list.Get(), &dst, dest, 0, 0, &src, &box, 0);
    }

    int create_placed(Frame& frame, const D3D12_RESOURCE_DESC& d, D3D12_RESOURCE_STATES state, size_t bytes) {
        placement.reset(XMemAlloc(bytes, placement_attributes));
        return place_at(frame, d, state, placement.get());
    }

    int place_at(Frame& frame, const D3D12_RESOURCE_DESC& d, D3D12_RESOURCE_STATES state, void* base) {
        constexpr size_t alignment = 64 * 1024;
        UINT64 address = reinterpret_cast<UINT64>(base);
        if (!address || address % alignment) {
            std::fputs("Expected a 64 KiB-aligned XMem placement address\n", stderr);
            return 1;
        }
        auto xbox_desc = d;
        xbox_desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(261); // Observed Xbox tile mode.
        using PlacedX = HRESULT (WINAPI*)(ID3D12Device*, UINT64, const D3D12_RESOURCE_DESC*,
            D3D12_RESOURCE_STATES, const D3D12_CLEAR_VALUE*, REFIID, void**);
        CHECK(xbox_method<PlacedX>(frame.dev.Get(), 44)(frame.dev.Get(), address, &xbox_desc,
              state, nullptr, IID_PPV_ARGS(&texture)));
        return 0;
    }

    // Walk the tile in storage order, expanding each logical sprite texel.
    // The runtime independently calculates a byte address from (x, y).
    void write_cpu_sprites(bool highlight) {
        auto* dst = static_cast<unsigned char*>(placement.get());
        for (UINT i = 0; i < cpu_width * cpu_height; ++i) {
            const UINT tile = i / 1024;
            const UINT x = (tile % (cpu_width / 32)) * 32 + (i & 3) + ((i >> 3) & 4)
                         + ((i >> 4) & 8) + ((i >> 5) & 16);
            const UINT y = (tile / (cpu_width / 32)) * 32 + ((i >> 2) & 7)
                         + ((i >> 3) & 8) + ((i >> 4) & 16);
            UINT u = x * width / cpu_width;
            Color texel = sprite_texel(Sprite(u / 4), u % 4, y * height / cpu_height, highlight);
            std::memcpy(dst + i * 4, texel.data(), 4);
        }
    }

    int init_cpu(Frame& frame, UINT w, UINT h, UINT16 levels, size_t bytes,
                 DXGI_FORMAT format = DXGI_FORMAT_R8G8B8A8_UNORM, UINT16 slices = 1) {
        D3D12_RESOURCE_DESC d = {};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = w;
        d.Height = h;
        d.DepthOrArraySize = slices;
        d.SampleDesc.Count = 1;
        d.MipLevels = levels;
        d.Format = format;
        if (create_placed(frame, d, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, bytes)) return 1;
        cpu_tiled = true;
        return create_views(frame);
    }

    void set_final_coin(Frame& frame, bool highlight) {
        if (final_coin_uploaded == highlight) return;
        final_coin_uploaded = highlight;
        if (cpu_tiled) {
            write_cpu_sprites(highlight);
            return;
        }
        transition(frame, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        constexpr UINT coin_column = 4 * Coin;
        copy_columns(frame, highlight ? width : coin_column, 4, coin_column);
        transition(frame, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
    }

    int init(Frame& frame, bool placed) {
        D3D12_HEAP_PROPERTIES hp = {};
        hp.Type = D3D12_HEAP_TYPE_DEFAULT;
        hp.CreationNodeMask = hp.VisibleNodeMask = 1;
        D3D12_RESOURCE_DESC d = {};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = width;
        d.Height = height;
        d.DepthOrArraySize = d.MipLevels = d.SampleDesc.Count = 1;
        d.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        if (placed) {
            if (create_placed(frame, d, D3D12_RESOURCE_STATE_COPY_DEST, 64 * 1024)) return 1;
        } else {
            CHECK(frame.dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d,
                  D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&texture)));
        }
        constexpr UINT staging_width = width + 4;
        d.Width = staging_width;
        UINT64 bytes;
        frame.dev->GetCopyableFootprints(&d, 0, 1, 0, &footprint, nullptr, nullptr, &bytes);
        hp.Type = D3D12_HEAP_TYPE_UPLOAD;
        d.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        d.Width = bytes;
        d.Height = 1;
        d.Format = DXGI_FORMAT_UNKNOWN;
        d.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        CHECK(frame.dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d,
              D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&upload)));
        void* mapped;
        D3D12_RANGE no_reads = {};
        CHECK(upload->Map(0, &no_reads, &mapped));
        for (UINT y = 0; y < height; ++y) {
            auto* row = static_cast<unsigned char*>(mapped) + y * footprint.Footprint.RowPitch;
            for (UINT x = 0; x < staging_width; ++x) {
                Color texel = sprite_texel(x < width ? Sprite(x / 4) : Coin, x % 4, y, x >= width);
                std::memcpy(row + x * 4, texel.data(), 4);
            }
        }
        upload->Unmap(0, nullptr);
        copy_columns(frame, 0, width, 0);
        transition(frame, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);

        return create_views(frame);
    }

    int create_views(Frame& frame) {
        D3D12_DESCRIPTOR_HEAP_DESC heap = {D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1,
                                           D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE};
        CHECK(frame.dev->CreateDescriptorHeap(&heap, IID_PPV_ARGS(&views)));
        heap.Type = D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
        CHECK(frame.dev->CreateDescriptorHeap(&heap, IID_PPV_ARGS(&samplers)));
        using Handle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
        // Xbox handle getters return the address in RAX.
        D3D12_CPU_DESCRIPTOR_HANDLE srv = {xbox_method<Handle>(views.Get(), 9)(views.Get())};
        frame.dev->CreateShaderResourceView(texture.Get(), nullptr, srv);
        D3D12_CPU_DESCRIPTOR_HANDLE sampler = {xbox_method<Handle>(samplers.Get(), 9)(samplers.Get())};
        D3D12_SAMPLER_DESC desc = {};
        desc.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
        desc.AddressU = desc.AddressV = desc.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        desc.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
        desc.MaxAnisotropy = 1;
        frame.dev->CreateSampler(&desc, sampler);
        tables[0].ptr = xbox_method<Handle>(views.Get(), 10)(views.Get());
        tables[1].ptr = xbox_method<Handle>(samplers.Get(), 10)(samplers.Get());
        return 0;
    }

    void bind(ID3D12GraphicsCommandList* list) {
        ID3D12DescriptorHeap* heaps[] = {views.Get(), samplers.Get()};
        using Heaps = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
        xbox_method<Heaps>(list, 32)(list, 2, heaps);
    }
};
