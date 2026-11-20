// Graphical frontend for the same Rust game used by the terminal example.
#define UNICODE
#include "graphics.h"
#include "triangle_shaders.h"
#include "coin_scene.h"
#include "coin_controller.h"
#include "bc1_reference.h"
#include <vector>
#include <algorithm>
#include <memory>

struct Game;
extern "C" {
    Game* coin_new();
    void coin_delete(Game*);
    bool coin_move(Game*, unsigned char);
    void coin_board(const Game*, Board*);
}

constexpr UINT tile_size = 64;

struct BoardRenderer {
    Frame frame;
    Mesh mesh;
    SpriteAtlas atlas;
    std::vector<Vertex> vertices;
    D3D12_SRV_DIMENSION view_dimension = D3D12_SRV_DIMENSION_TEXTURE2D;

    UINT64 checked_frames = 0;
    int init(const Board& board, bool placed, bool cpu_tiled, bool scene_3d) {
        if (scene_3d) {
            if (frame.init(512, 384, true) || atlas.init(frame, false)) return 1;
            return init_mesh(board.width * board.height * scene::max_vertices_per_tile);
        }
        if (frame.init(UINT(board.width) * tile_size, UINT(board.height) * tile_size)) return 1;
        if (cpu_tiled) {
            if (atlas.init_cpu(frame, atlas.cpu_width, atlas.cpu_height, 1, atlas.cpu_width * atlas.cpu_height * 4)) return 1;
            atlas.write_cpu_sprites(false);
        } else if (atlas.init(frame, placed)) return 1;
        return init_mesh(board.width * board.height * 6);
    }

    int init_mesh(size_t vertex_count) {
        const D3D12_DESCRIPTOR_RANGE ranges[] = {
            {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 0, 0, 0}, {D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER, 1, 0, 0, 0}
        };
        D3D12_ROOT_PARAMETER tables[2]{};
        for (UINT i = 0; i < 2; ++i) {
            tables[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
            tables[i].DescriptorTable = {1, &ranges[i]};
            tables[i].ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
        }
        const D3D12_INPUT_ELEMENT_DESC inputs[] = {
            {"POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 0, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
            {"TEXCOORD", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 16, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        };
        vertices.resize(vertex_count);
        D3D12_SHADER_BYTECODE ps = {board_ps, sizeof(board_ps)};
        if (view_dimension == D3D12_SRV_DIMENSION_TEXTURE2DARRAY) ps = {array_ps, sizeof(array_ps)};
        if (view_dimension == D3D12_SRV_DIMENSION_TEXTURECUBE) ps = {cube_ps, sizeof(cube_ps)};
        D3D12_SHADER_BYTECODE vs = {board_vs, sizeof(board_vs)};
        if (frame.depth) {
            vs = {scene_vs, sizeof(scene_vs)};
            ps = {scene_ps, sizeof(scene_ps)};
        }
        return mesh.init(frame.dev.Get(), vs, ps,
                         inputs, 2, UINT(vertices.size() * sizeof(Vertex)), tables, 2, frame.depth.Get() != nullptr);
    }

    int render(const Board& board, bool present, bool verify_scene = false) {
        if (frame.depth) {
            scene::build(board, vertices);
            if (frame.begin({0.075f, 0.095f, 0.14f, 1})) return 1;
            if (board.coins) atlas.set_final_coin(frame, board.coins == 1);
            atlas.bind(frame.list.Get());
            if (mesh.draw(frame, vertices.data(), UINT(vertices.size()), sizeof(Vertex), atlas.tables, 2) ||
                frame.finish(present)) return 1;
            ++checked_frames;
            return verify_scene ? scene::check_pixels(frame, board) : 0;
        }
        for (size_t i = 0; i < board.width * board.height; ++i) {
            Sprite sprite = sprite_at(board, i);
            constexpr float inset = 0.04f;
            constexpr int corners[][2] = {{0,0}, {1,0}, {1,1}, {0,0}, {1,1}, {0,1}};
            for (size_t j = 0; j < 6; ++j) {
                float x = float(i % board.width)  + (corners[j][0] ? 1 - inset : inset);
                float y = float(i / board.width) + (corners[j][1] ? 1 - inset : inset);
                vertices[6*i+j] = {2*x/board.width-1, 1-2*y/board.height, 0, 1,
                                   {(float(sprite) + corners[j][0]) / 4.f, float(corners[j][1]), 0, 1}};
            }
        }
        if (atlas.cpu_tiled && (!frame.frame || (board.coins && atlas.final_coin_uploaded != (board.coins == 1)))) {
            atlas.set_final_coin(frame, board.coins == 1);
            // PresentX submits the mirror on the same queue before the next draw.
            // On a pickup, present the previous board while the texture changes.
            if (frame.frame) {
                if (frame.present_target()) return 1;
            } else if (frame.begin({0, 0, 0, 1}) || frame.finish(true)) return 1;
        }
        if (frame.begin({0.035f, 0.05f, 0.07f, 1})) return 1;
        if (!atlas.cpu_tiled && board.coins) atlas.set_final_coin(frame, board.coins == 1);
        atlas.bind(frame.list.Get());
        if (mesh.draw(frame, vertices.data(), UINT(vertices.size()), sizeof(Vertex), atlas.tables, 2) ||
            frame.finish(present)) return 1;

        D3D12_RANGE range = {0, size_t(frame.height) * frame.pitch};
        void* mapped = nullptr;
        CHECK(frame.readback->Map(0, &range, &mapped));
        bool pass = true;
        for (size_t y = 0; y < board.height; ++y) {
            for (size_t x = 0; x < board.width; ++x) {
                Sprite sprite = sprite_at(board, y * board.width + x);
                for (UINT v = 0; v < 4; ++v) {
                    for (UINT u = 0; u < 4; ++u) {
                        // These fixed sample positions lie inside each magnified texel,
                        // including the small gutter around the quad.
                        size_t px = x * tile_size + 8 + u * 16;
                        size_t py = y * tile_size + 8 + v * 16;
                        const auto* pixel = static_cast<const unsigned char*>(mapped) + py * frame.pitch + px * 4;
                        Color expected = sprite_texel(sprite, u, v, board.coins == 1);
                        if (std::memcmp(pixel, expected.data(), 4)) {
                            if (pass) std::fprintf(stderr,
                                "Frame %llu tile (%zu,%zu) texel (%u,%u): got %u %u %u %u, expected %u %u %u %u\n",
                                frame.frame, x, y, u, v, pixel[0], pixel[1], pixel[2], pixel[3],
                                expected[0], expected[1], expected[2], expected[3]);
                            pass = false;
                        }
                    }
                }
            }
        }
        D3D12_RANGE no_writes = {};
        frame.readback->Unmap(0, &no_writes);
        ++checked_frames;
        return pass ? 0 : 1;
    }
};

// Each draw uses an SRV exposing exactly one mip. The preceding frame's
// fence has completed before we overwrite the descriptor.
static int sample_mip(BoardRenderer& renderer, ID3D12Resource* texture, DXGI_FORMAT format,
                      UINT level, Color* sampled, UINT slice = 0, UINT slices = 1) {
    auto& frame = renderer.frame;
    auto& atlas = renderer.atlas;
    D3D12_SHADER_RESOURCE_VIEW_DESC srv = {};
    srv.Format = format;
    srv.ViewDimension = renderer.view_dimension;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    if (srv.ViewDimension == D3D12_SRV_DIMENSION_TEXTURECUBE) {
        srv.TextureCube.MostDetailedMip = level;
        srv.TextureCube.MipLevels = 1;
    } else if (srv.ViewDimension == D3D12_SRV_DIMENSION_TEXTURE2DARRAY) {
        srv.Texture2DArray.MostDetailedMip = level;
        srv.Texture2DArray.MipLevels = 1;
        srv.Texture2DArray.ArraySize = slices;
    } else {
        srv.Texture2D.MostDetailedMip = level;
        srv.Texture2D.MipLevels = 1;
    }
    using Handle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
    D3D12_CPU_DESCRIPTOR_HANDLE handle = {xbox_method<Handle>(atlas.views.Get(), 9)(atlas.views.Get())};
    frame.dev->CreateShaderResourceView(texture, &srv, handle);
    if (frame.begin({0,0,0,1})) return 1;
    Vertex quad[] = {
        {-1,1,0,1,{0,0}}, {1,1,0,1,{1,0}}, {1,-1,0,1,{1,1}},
        {-1,1,0,1,{0,0}}, {1,-1,0,1,{1,1}}, {-1,-1,0,1,{0,1}},
    };
    for (auto& vertex : quad) {
        if (srv.ViewDimension == D3D12_SRV_DIMENSION_TEXTURECUBE) {
            // World-space directions to the corners of each face in D3D order.
            // Use a different major-axis magnitude per face to exercise projection.
            const float major = float(slice + 1);
            const float u = (2 * vertex.uv[0] - 1) * major;
            const float v = (2 * vertex.uv[1] - 1) * major;
            const float directions[6][3] = {
                {major, -v, -u}, {-major, -v, u}, {u, major, v},
                {u, -major, -v}, {u, -v, major}, {-u, -v, -major},
            };
            std::copy_n(directions[slice], 3, vertex.uv);
        } else {
            vertex.uv[2] = float(slice);
        }
    }
    atlas.bind(frame.list.Get());
    if (renderer.mesh.draw(frame, quad, 6, sizeof(Vertex), atlas.tables, 2) || frame.finish(true)) return 1;
    void* mapped = nullptr;
    D3D12_RANGE read = {0, size_t(frame.pitch) * frame.height};
    CHECK(frame.readback->Map(0, &read, &mapped));
    UINT width = (std::max)(1u, frame.width >> level);
    UINT height = (std::max)(1u, frame.height >> level);
    UINT scale_x = frame.width / width, scale_y = frame.height / height;
    for (UINT y = 0; y < height; ++y) {
        auto* row = static_cast<const unsigned char*>(mapped) + size_t(frame.pitch) * (y * scale_y + scale_y / 2);
        for (UINT x = 0; x < width; ++x) {
            auto* pixel = row + 4 * (x * scale_x + scale_x / 2);
            std::memcpy(sampled[y*width+x].data(), pixel, sizeof(Color));
        }
    }
    D3D12_RANGE no_writes = {};
    frame.readback->Unmap(0, &no_writes);
    return 0;
}

// Keep mip 1 in COPY_SOURCE across PresentX while mip 0 remains shader-readable.
static int check_texture_states() {
    BoardRenderer renderer;
    auto& frame = renderer.frame;
    auto& atlas = renderer.atlas;
    if (frame.init(32, 32) || atlas.init_cpu(frame, 32, 32, 2, 8192) || renderer.init_mesh(6)) return 1;

    constexpr Color base = {79, 132, 187, 255}, tail = {18, 57, 96, 255}, edit = {255, 93, 28, 255};
    auto* memory = static_cast<unsigned char*>(atlas.placement.get());
    // One packed tail tile followed by mip 0. The 16x16 mip starts at byte 2048.
    for (UINT offset = 0; offset < 8192; offset += 4) {
        std::memcpy(memory + offset, (offset < 4096 ? tail : base).data(), 4);
    }
    if (frame.begin({0,0,0,1}) || frame.finish(true)) return 1;

    auto transition_mip = [&](ID3D12GraphicsCommandList* list, D3D12_RESOURCE_STATES before,
                              D3D12_RESOURCE_STATES after) {
        D3D12_RESOURCE_BARRIER b = {};
        b.Transition = {atlas.texture.Get(), 1, before, after};
        list->ResourceBarrier(1, &b);
    };
    // Record the second list first. Only submission order gives COPY_SOURCE.
    CHECK(frame.alloc->Reset());
    ComPtr<ID3D12GraphicsCommandList> second;
    if (frame.create_list(second.GetAddressOf())) return 1;
    transition_mip(second.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
    CHECK(second->Close());
    CHECK(frame.list->Reset(frame.alloc.Get(), nullptr));
    transition_mip(frame.list.Get(), D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    CHECK(frame.list->Close());
    ID3D12CommandList* work[] = {frame.list.Get(), second.Get()};
    frame.queue->ExecuteCommandLists(2, work);
    std::memcpy(memory + 2048, edit.data(), 4);
    if (frame.present_target() || frame.wait_for_gpu()) return 1;

    std::vector<Color> samples(32 * 32);
    if (sample_mip(renderer, atlas.texture.Get(), DXGI_FORMAT_R8G8B8A8_UNORM, 0, samples.data())) return 1;
    if (!std::all_of(samples.begin(), samples.end(), [&](const Color& c) { return c == base; })) {
        std::fputs("Shader readback mismatch on neighboring mip 0\n", stderr);
        return 1;
    }

    CHECK(frame.alloc->Reset());
    CHECK(frame.list->Reset(frame.alloc.Get(), nullptr));
    D3D12_TEXTURE_COPY_LOCATION src = {}, dst = {};
    src.pResource = atlas.texture.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    src.SubresourceIndex = 1;
    dst.pResource = frame.readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint.Footprint = {DXGI_FORMAT_R8G8B8A8_UNORM, 16, 16, 1, frame.pitch};
    using CopyX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                 UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
    xbox_method<CopyX>(frame.list.Get(), 83)(frame.list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
    CHECK(frame.list->Close());
    frame.queue->ExecuteCommandLists(1, work);
    if (frame.wait_for_gpu()) return 1;
    D3D12_RANGE read = {0, size_t(frame.pitch) * 16};
    void* mapped = nullptr;
    CHECK(frame.readback->Map(0, &read, &mapped));
    bool match = true;
    for (UINT y = 0; y < 16; ++y) {
        auto* row = static_cast<unsigned char*>(mapped) + y * frame.pitch;
        for (UINT x = 0; x < 16; ++x) {
            const auto& expected = x == 0 && y == 0 ? edit : tail;
            if (std::memcmp(row + 4*x, expected.data(), 4)) match = false;
        }
    }
    D3D12_RANGE no_writes = {};
    frame.readback->Unmap(0, &no_writes);
    if (!match) { std::fputs("COPY_SOURCE readback mismatch on mip 1\n", stderr); return 1; }
    std::puts("Verified 1024 sampled pixels on mip 0 and 256 copied pixels on mip 1 after the CPU edit.");
    return 0;
}

extern "C" __declspec(dllimport) void* WINAPI XMemVirtualAlloc(void*, size_t, UINT, UINT, UINT64);
extern "C" __declspec(dllimport) int WINAPI XMemAllocatePhysicalPages(UINT64, size_t*, UINT64*);
extern "C" __declspec(dllimport) void* WINAPI XMemMapPhysicalPages(void*, size_t, UINT64*);

static int check_streamed_mips(const char* path) {
    constexpr UINT width = 256;
    constexpr size_t page = 65536, pages = 5, fine_pixels = width * width;
    std::vector<unsigned char> tiled(pages * page);
    std::vector<Color> expected(fine_pixels + fine_pixels / 4);
    FILE* file = nullptr;
    fopen_s(&file, path, "rb");
    std::unique_ptr<FILE, decltype(&std::fclose)> input(file, std::fclose);
    if (!input || std::fread(tiled.data(), 1, tiled.size(), file) != tiled.size() ||
        std::fread(expected.data(), sizeof(Color), expected.size(), file) != expected.size() ||
        std::fgetc(file) != EOF) {
        std::fprintf(stderr, "Invalid streamed texture fixture: %s\n", path);
        return 1;
    }

    // Keep the reservation alive until the renderer releases its texture.
    const auto release = [](unsigned char* ptr) { VirtualFree(ptr, 0, MEM_RELEASE); };
    std::unique_ptr<unsigned char, decltype(release)> memory(static_cast<unsigned char*>(
        XMemVirtualAlloc(nullptr, tiled.size(), MEM_RESERVE, PAGE_NOACCESS, 0)), release);
    std::array<UINT64, pages> tokens{};
    size_t count = pages;
    if (!memory || !XMemAllocatePhysicalPages(0, &count, tokens.data()) || count != pages) {
        std::fputs("Could not reserve placement memory and allocate five XMem pages\n", stderr);
        return 1;
    }
    auto map_pages = [&](UINT first, UINT n) {
        auto* address = memory.get() + first * page;
        if (XMemMapPhysicalPages(address, n, &tokens[first]) != address) {
            std::fprintf(stderr, "Failed to map %u pages at page %u\n", n, first);
            return false;
        }
        std::memcpy(address, tiled.data() + first * page, n * page);
        return true;
    };
    auto check_residency = [&](UINT mask) {
        for (UINT i = 0; i < pages; ++i) {
            MEMORY_BASIC_INFORMATION info{};
            DWORD wanted = mask & (1u << i) ? MEM_COMMIT : MEM_RESERVE;
            if (!VirtualQuery(memory.get() + i * page, &info, sizeof(info)) || info.State != wanted) {
                std::fprintf(stderr, "Unexpected residency at page %u, expected mask %#x\n", i, mask);
                return false;
            }
        }
        return true;
    };

    BoardRenderer renderer;
    auto& frame = renderer.frame;
    auto& atlas = renderer.atlas;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = width;
    desc.DepthOrArraySize = desc.SampleDesc.Count = 1;
    desc.MipLevels = 2;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    if (frame.init(width, width) ||
        atlas.place_at(frame, desc, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, memory.get()) ||
        atlas.create_views(frame) || renderer.init_mesh(6)) return 1;
    std::vector<Color> pixels(fine_pixels);
    auto draw_mip = [&](UINT level, const char* phase) {
        if (sample_mip(renderer, atlas.texture.Get(), desc.Format, level, pixels.data())) return false;
        const UINT side = width >> level;
        const size_t start = level ? fine_pixels : 0;
        for (UINT i = 0; i < side * side; ++i) {
            if (pixels[i] != expected[start + i]) {
                std::fprintf(stderr, "%s: mip %u mismatch at (%u,%u)\n", phase, level, i % side, i / side);
                return false;
            }
        }
        return true;
    };

    if (!map_pages(0, 1) || !check_residency(0x01) || frame.begin({0,0,0,1}) || frame.finish(true) ||
        !draw_mip(1, "coarse only")) return 1;
    // Both ends of mip 0 are readable, but two pages in its interior are reserved.
    if (!map_pages(1, 1) || !map_pages(4, 1) || !check_residency(0x13) ||
        !draw_mip(1, "partial detail")) return 1;
    if (!map_pages(2, 2) || !check_residency(0x1f) || frame.present_target() ||
        !draw_mip(0, "full detail") || !draw_mip(1, "coarse after streaming")) return 1;

    auto* detail = memory.get() + page;
    if (XMemMapPhysicalPages(detail, 4, nullptr) != detail || !check_residency(0x01) ||
        !draw_mip(1, "evicted detail")) return 1;
    // Change the blue channel on every fine texel for the reload at the same address.
    for (size_t i = page + 2; i < tiled.size(); i += 4) tiled[i] ^= 255;
    for (size_t i = 0; i < fine_pixels; ++i) expected[i][2] ^= 255;
    if (!map_pages(1, 4) || !check_residency(0x1f) || frame.present_target() ||
        !draw_mip(0, "reloaded detail") || !draw_mip(1, "coarse after reload")) return 1;

    std::puts("Verified coarse rendering, a partially mapped mip, full detail, eviction and replacement at the same address.");
    return 0;
}

struct ReferenceCase {
    const char* option;
    UINT width, height;
    UINT16 levels;
    DXGI_FORMAT format;
    size_t placed_bytes, last_mip_offset;
    bool compare_committed_bc1 = false;
    UINT16 slices = 1;
    D3D12_SRV_DIMENSION view_dimension = D3D12_SRV_DIMENSION_TEXTURE2D;
};

static constexpr ReferenceCase references[] = {
    {"--cube-test", 64, 64, 7, DXGI_FORMAT_R8G8B8A8_UNORM, 147456, 768, false, 6, D3D12_SRV_DIMENSION_TEXTURECUBE},
    {"--tiled-reference", 256, 256, 1, DXGI_FORMAT_R8G8B8A8_UNORM, 262144, 0},
    {"--mip-reference", 256, 256, 9, DXGI_FORMAT_R8G8B8A8_UNORM, 352256, 768},
    {"--bc1-reference", 256, 256, 9, DXGI_FORMAT_BC1_UNORM, 45056, 256, true},
    {"--array-rgba-tall", 64, 256, 9, DXGI_FORMAT_R8G8B8A8_UNORM, 282624, 512, false, 3, D3D12_SRV_DIMENSION_TEXTURE2DARRAY},
    {"--array-bc1-wide", 256, 64, 9, DXGI_FORMAT_BC1_UNORM, 49152, 256, false, 3, D3D12_SRV_DIMENSION_TEXTURE2DARRAY},
    {"--rect-rgba-wide", 256, 64, 9, DXGI_FORMAT_R8G8B8A8_UNORM, 98304, 768},
    {"--rect-rgba-tall", 64, 256, 9, DXGI_FORMAT_R8G8B8A8_UNORM, 94208, 512},
    {"--rect-bc1-wide", 256, 64, 9, DXGI_FORMAT_BC1_UNORM, 16384, 256},
    {"--rect-bc1-tall", 128, 256, 9, DXGI_FORMAT_BC1_UNORM, 28672, 256},
};

static int check_reference(const char* path, const ReferenceCase& ref) {
    const UINT16 levels = ref.levels;
    constexpr const char* face_names[] = {"+X", "-X", "+Y", "-Y", "+Z", "-Z"};
    const bool cube = ref.view_dimension == D3D12_SRV_DIMENSION_TEXTURECUBE;
    const bool bc1 = ref.format == DXGI_FORMAT_BC1_UNORM;
    auto mip_width = [&](UINT mip) { return (std::max)(1u, ref.width >> mip); };
    auto mip_pixels = [&](UINT mip) { return mip_width(mip) * (std::max)(1u, ref.height >> mip); };
    FILE* file = nullptr;
    fopen_s(&file, path, "rb");
    std::unique_ptr<FILE, decltype(&std::fclose)> input(file, std::fclose);
    if (!input) { std::perror(path); return 1; }
    BoardRenderer renderer;
    renderer.view_dimension = ref.view_dimension;
    auto& frame = renderer.frame;
    auto& atlas = renderer.atlas;
    if (frame.init(ref.width, ref.height) || atlas.init_cpu(frame, ref.width, ref.height, levels, ref.placed_bytes, ref.format, ref.slices) ||
        renderer.init_mesh(6)) return 1;
    size_t slice_pixels = 0;
    for (UINT mip = 0; mip < levels; ++mip) slice_pixels += mip_pixels(mip);
    const size_t pixels = slice_pixels * ref.slices;
    std::vector<Color> expected(pixels);
    Bc1Reference linear;
    std::vector<unsigned char> blocks(ref.compare_committed_bc1 ? 43704 : 0); // 5,463 row-major BC1 blocks.
    void* reference_data = ref.compare_committed_bc1 ? static_cast<void*>(blocks.data()) : expected.data();
    size_t reference_bytes = ref.compare_committed_bc1 ? blocks.size() : pixels * sizeof(Color);
    if (std::fread(atlas.placement.get(), 1, ref.placed_bytes, input.get()) != ref.placed_bytes ||
        std::fread(reference_data, 1, reference_bytes, input.get()) != reference_bytes || std::fgetc(input.get()) != EOF) {
        std::fputs("Invalid tiled reference fixture length\n", stderr);
        return 1;
    }
    if (ref.compare_committed_bc1 && linear.init(frame, blocks.data(), blocks.size())) return 1;
    if (frame.begin({0,0,0,1}) || frame.finish(true)) return 1;
    if (ref.compare_committed_bc1) {
        size_t start = 0;
        for (UINT mip = 0; mip < levels; ++mip) {
            if (sample_mip(renderer, linear.texture.Get(), ref.format, mip, &expected[start])) return 1;
            start += mip_pixels(mip);
        }
    }
    std::vector<Color> sampled(ref.width * ref.height);
    auto check_levels = [&]() {
        size_t start = 0;
        for (UINT slice = 0; slice < ref.slices; ++slice) {
            for (UINT mip = 0; mip < levels; ++mip) {
                UINT width = mip_width(mip);
                UINT count = mip_pixels(mip);
                if (sample_mip(renderer, atlas.texture.Get(), ref.format, mip, sampled.data(), slice, ref.slices)) return false;
                for (UINT i = 0; i < count; ++i) {
                    if (sampled[i] != expected[start + i]) {
                        if (cube) std::fprintf(stderr, "Face %s: ", face_names[slice]);
                        std::fprintf(stderr, "%s mismatch at (%u,%u), mip %u, slice %u\n", ref.option, i % width, i / width, mip, slice);
                        return false;
                    }
                }
                start += count;
            }
        }
        return true;
    };
    if (!check_levels()) return 1;
    std::printf("Verified %zu/%zu sampled pixels across %u %s mip level(s), %u %s, %ux%u base.\n",
                pixels, pixels, levels, bc1 ? "BC1" : "RGBA8", ref.slices,
                cube ? "cube faces" : "slice(s)", ref.width, ref.height);
    if (levels > 1) {
        auto* memory = static_cast<unsigned char*>(atlas.placement.get());
        const UINT slice = ref.slices / 2;
        const size_t offset = slice * (ref.placed_bytes / ref.slices) + ref.last_mip_offset;
        Color& last_pixel = expected[(slice + 1) * slice_pixels - 1];
        if (bc1) {
            // Equal endpoints plus selector 3 yield transparent black for the entire block.
            const unsigned char transparent[] = {0, 0, 0, 0, 255, 255, 255, 255};
            std::memcpy(memory + offset, transparent, sizeof(transparent));
            last_pixel = {0, 0, 0, 0};
        } else {
            memory[offset] ^= 255; // Red byte at the RGBA fixture's 1x1 mip origin.
            last_pixel[0] ^= 255;
        }
        if (frame.present_target() || !check_levels()) return 1;
        if (cube) std::printf("Verified the 1x1 CPU update on the %s face", face_names[slice]);
        else std::printf("Verified the 1x1 CPU update in slice %u", slice);
        std::printf(" and all %u neighboring subresources.\n", levels * ref.slices - 1);
    }
    return 0;
}

static HWND game_window() {
    WNDCLASSW wc = {};
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.lpszClassName = L"TomBoxCoinRun";
    if (!RegisterClassW(&wc)) return nullptr;
    return CreateWindowExW(0, wc.lpszClassName, L"Coin Run", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                           CW_USEDEFAULT, CW_USEDEFAULT, 640, 480, nullptr, nullptr, wc.hInstance, nullptr);
}

int main(int argc, char** argv) {
    const bool gameinput_test = argc == 2 && !std::strcmp(argv[1], "--gameinput-test");
    const bool controller_test = gameinput_test || (argc == 2 && !std::strcmp(argv[1], "--controller-test"));
    const bool scene_test = argc == 2 && !std::strcmp(argv[1], "--3d-test");
    const bool scene_3d = controller_test || scene_test || (argc == 2 && !std::strcmp(argv[1], "--3d"));
    const bool stream_test = argc == 3 && !std::strcmp(argv[1], "--stream-test");
    const bool state_test = argc == 2 && !std::strcmp(argv[1], "--state-test");
    const bool placed = argc == 2 && !std::strcmp(argv[1], "--placed-test");
    const bool tiled_test = argc == 2 && !std::strcmp(argv[1], "--tiled-test");
    const bool cpu_tiled = tiled_test || (argc == 2 && !std::strcmp(argv[1], "--tiled"));
    const ReferenceCase* reference = nullptr;
    if (argc == 3) {
        for (const auto& candidate : references) {
            if (!std::strcmp(argv[1], candidate.option)) reference = &candidate;
        }
    }
    const bool headless = placed || (argc == 2 && !std::strcmp(argv[1], "--test"));
    const bool present_test = argc == 2 && !std::strcmp(argv[1], "--present-test");
    if (argc != 1 && !headless && !present_test && !cpu_tiled && !reference && !state_test && !stream_test && !scene_3d) {
        std::fputs("usage: coin_window.exe [--test | --placed-test | --present-test | --tiled | --tiled-test | --tiled-reference FILE | --mip-reference FILE | --bc1-reference FILE | --rect-{rgba,bc1}-{wide,tall} FILE | --array-{rgba-tall,bc1-wide} FILE | --cube-test FILE | --state-test | --stream-test FILE | --3d | --3d-test | --controller-test | --gameinput-test]\n", stderr);
        return 1;
    }
    HWND window = headless ? nullptr : game_window();
    if (!headless && !window) { std::fputs("Window creation failed\n", stderr); return 1; }
    if (reference || state_test || stream_test) {
        int status;
        if (stream_test) status = check_streamed_mips(argv[2]);
        else if (state_test) status = check_texture_states();
        else status = check_reference(argv[2], *reference);
        DestroyWindow(window);
        return status;
    }
    std::unique_ptr<Game, decltype(&coin_delete)> game(coin_new(), coin_delete);
    if (!game) return 1;
    Board board;
    coin_board(game.get(), &board);
    BoardRenderer renderer;
    if (renderer.init(board, placed, cpu_tiled, scene_3d)) return 1;
    auto paint = [&]() {
        coin_board(game.get(), &board);
        if (window) {
            wchar_t title[160];
            if (board.coins) swprintf_s(title, L"Coin Run  |  WASD / D-pad / left stick  |  %zu left  |  R / Menu to restart  |  Q to quit", board.coins);
            else wcscpy_s(title, L"Coin Run  |  Both coins collected!  |  R / Menu to replay  |  Q to quit");
            SetWindowTextW(window, title);
        }
        return renderer.render(board, window != nullptr, scene_test || controller_test);
    };
    if (paint()) return 1;
    if (headless || present_test || tiled_test || scene_test) {
        // Independent expected states for the canonical map, including blocked moves.
        // The mask tracks the top-right coin (1) and bottom-left coin (2).
        struct Step { char key; unsigned player, mask; bool changed; };
        const Step route[] = {
            {'a',0,3,false}, {'d',1,3,true}, {'d',2,3,true}, {'s',2,3,false},
            {'d',3,2,true}, {'a',2,2,true}, {'a',1,2,true}, {'a',0,2,true},
            {'s',4,2,true}, {'s',8,0,true}, {'d',8,0,false},
            {'r',0,3,true}, {'s',4,3,true}, {'s',8,1,true}, {'w',4,1,true},
            {'w',0,1,true}, {'d',1,1,true}, {'d',2,1,true}, {'d',3,0,true},
        };
        if (board.player != 0 || board.coins != 2) return 1;
        for (const auto& step : route) {
            bool changed = coin_move(game.get(), step.key);
            if (paint()) return 1;
            unsigned coins = (step.mask & 1) + (step.mask >> 1);
            if (changed != step.changed || board.player != step.player || board.coins != coins ||
                board.tiles[3] != (step.mask & 1 ? '*' : '.') ||
                board.tiles[8] != (step.mask & 2 ? '*' : '.')) {
                std::fprintf(stderr, "Game state mismatch at frame %llu after %c\n", renderer.frame.frame, step.key);
                return 1;
            }
        }
        if (scene_test) std::printf("Verified perspective, textured faces and depth occlusion across %llu frames, both pickup orders and restart.\n",
                                   renderer.checked_frames);
        else std::printf("Verified %s atlas, %llu frames, %zu sprite samples per frame, both pickup orders.\n",
                    cpu_tiled ? "CPU-tiled" : (placed ? "placed" : "committed"), renderer.checked_frames, board.width * board.height * 16);
    } else {
        CoinController pad(!gameinput_test);
        constexpr UINT_PTR poll_timer = 1;
        if (!SetTimer(window, poll_timer, 16, nullptr)) return 1;
        bool ready = false;
        ULONGLONG last_action = GetTickCount64();
        if (controller_test) {
            SetForegroundWindow(window);
            std::printf("Waiting for a controller (%s)\n", pad.backend());
            std::fflush(stdout);
        }
        MSG msg;
        while (IsWindow(window)) {
            int status = GetMessageW(&msg, nullptr, 0, 0);
            if (status < 0) return 1;
            if (!status) break;
            if (msg.message == WM_TIMER && msg.hwnd == window && msg.wParam == poll_timer) {
                if (!pad.available()) {
                    std::fputs("The GameInput-only test needs a working GameInput gamepad provider\n", stderr);
                    return 1;
                }
                if (controller_test && GetTickCount64() - last_action >= 30000) {
                    std::fputs("Timed out waiting for controller input\n", stderr);
                    return 1;
                }
                const bool was_pressed = pad.pressed();
                char action = 0;
                if (GetForegroundWindow() == window) action = pad.poll();
                else pad.reset();
                if (controller_test && ready != pad.ready()) {
                    ready = pad.ready();
                    std::printf("pad %s %s\n", ready ? "ready" : "inactive", pad.backend());
                    std::fflush(stdout);
                } else if (controller_test && ready && was_pressed && !pad.pressed()) {
                    std::puts("pad neutral");
                    std::fflush(stdout);
                }
                if (action) {
                    const auto previous_coins = board.coins;
                    if (coin_move(game.get(), action) && paint()) return 1;
                    if (board.coins < previous_coins) pad.pickup();
                    if (controller_test) {
                        last_action = GetTickCount64();
                        std::printf("pad %c %zu %zu\n", action, board.player, board.coins);
                        std::fflush(stdout);
                        if (previous_coins == 0 && action == 'r') break;
                    }
                }
                continue;
            }
            if (msg.message == WM_KEYDOWN && (msg.hwnd == window || IsChild(window, msg.hwnd))) {
                if (msg.wParam == 'Q' || msg.wParam == VK_ESCAPE) break;
                if (coin_move(game.get(), static_cast<unsigned char>(msg.wParam)) && paint()) return 1;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if (msg.hwnd == window && msg.message == WM_PAINT && !IsIconic(window) &&
                renderer.render(board, true)) return 1;
        }
        KillTimer(window, poll_timer);
    }
    if (IsWindow(window)) DestroyWindow(window);
    return 0;
}
