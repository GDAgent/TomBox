#pragma once
#include "graphics.h"

// Upload the independently verified, row-major BC1 stream to a committed PC
// texture. Sampling both textures on the same GPU keeps BC1 interpolation
// identical across drivers without adding a tolerance to the pixel comparison.
struct Bc1Reference {
    ComPtr<ID3D12Resource> texture, upload;

    int init(Frame& frame, const unsigned char* blocks, size_t size) {
        constexpr UINT levels = 9;
        D3D12_RESOURCE_DESC d = {};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = d.Height = 256;
        d.MipLevels = levels;
        d.DepthOrArraySize = d.SampleDesc.Count = 1;
        d.Format = DXGI_FORMAT_BC1_UNORM;
        D3D12_HEAP_PROPERTIES heap = {};
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        heap.Type = D3D12_HEAP_TYPE_DEFAULT;
        CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &d,
              D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&texture)));

        D3D12_PLACED_SUBRESOURCE_FOOTPRINT layouts[levels];
        UINT rows[levels];
        UINT64 row_bytes[levels], bytes;
        frame.dev->GetCopyableFootprints(&d, 0, levels, 0, layouts, rows, row_bytes, &bytes);
        size_t needed = 0;
        for (UINT mip = 0; mip < levels; ++mip) needed += rows[mip] * row_bytes[mip];
        if (size != needed) { std::fputs("Invalid linear BC1 reference size\n", stderr); return 1; }

        heap.Type = D3D12_HEAP_TYPE_UPLOAD;
        d.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        d.Width = bytes;
        d.Height = d.MipLevels = 1;
        d.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        d.Format = DXGI_FORMAT_UNKNOWN;
        CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &d,
              D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&upload)));
        void* mapped = nullptr;
        D3D12_RANGE no_reads = {};
        CHECK(upload->Map(0, &no_reads, &mapped));
        for (UINT mip = 0; mip < levels; ++mip) {
            auto* dst = static_cast<unsigned char*>(mapped) + layouts[mip].Offset;
            for (UINT y = 0; y < rows[mip]; ++y) {
                std::memcpy(dst + y * layouts[mip].Footprint.RowPitch, blocks, row_bytes[mip]);
                blocks += row_bytes[mip];
            }
        }
        upload->Unmap(0, nullptr);

        using CopyX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                     UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
        for (UINT mip = 0; mip < levels; ++mip) {
            D3D12_TEXTURE_COPY_LOCATION src = {}, dst = {};
            src.pResource = upload.Get();
            src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
            src.PlacedFootprint = layouts[mip];
            dst.pResource = texture.Get();
            dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
            dst.SubresourceIndex = mip;
            xbox_method<CopyX>(frame.list.Get(), 83)(frame.list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
        }
        D3D12_RESOURCE_BARRIER barrier = {};
        barrier.Transition = {texture.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                              D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE};
        frame.list->ResourceBarrier(1, &barrier);
        return 0;
    }
};
