// Graphical frontend for the same Rust game used by the terminal example.
#define UNICODE
#include "graphics.h"
#include "triangle_shaders.h"
#include "compute_probe.h"
#include "coin_scene.h"
#include "coin_controller.h"
#include "coin_audio.h"
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

template<typename ExpectedPixel>
static int check_readback(Frame& frame, const char* operation, UINT slot, ExpectedPixel expected_pixel) {
    void* mapped = nullptr;
    D3D12_RANGE read{0, size_t(frame.pitch) * frame.height};
    CHECK(frame.readback->Map(0, &read, &mapped));
    unsigned mismatches = 0;
    for (LONG y = 0; y < LONG(frame.height); ++y) {
        for (LONG x = 0; x < LONG(frame.width); ++x) {
            const auto expected = expected_pixel(x, y);
            const auto* pixel = static_cast<const BYTE*>(mapped) + y * frame.pitch + 4 * x;
            if (std::memcmp(pixel, expected.data(), 4) == 0) continue;
            if (++mismatches == 1) {
                std::fprintf(stderr, "Slot %u at (%ld,%ld): got %u %u %u %u, expected %u %u %u %u\n",
                             slot, x, y, pixel[0], pixel[1], pixel[2], pixel[3],
                             expected[0], expected[1], expected[2], expected[3]);
            }
        }
    }
    D3D12_RANGE no_writes{};
    frame.readback->Unmap(0, &no_writes);
    std::printf("%s slot %u: %u/%u texels match\n", operation, slot,
                frame.width * frame.height - mismatches, frame.width * frame.height);
    return mismatches != 0;
}

static int check_rect_readback(Frame& frame, const char* operation, UINT slot, const D3D12_RECT& rect,
                               Color expected_base, Color expected_inside) {
    return check_readback(frame, operation, slot, [&](LONG x, LONG y) {
        return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom ? expected_inside : expected_base;
    });
}

// Use separate CPU-only and shader-visible copies of the same UAV descriptor.
template<typename T>
static int check_uav_clear(Frame& frame, UINT slot, DXGI_FORMAT format,
                           const T (&base)[4], const T (&inside)[4],
                           Color expected_base, Color expected_inside) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = frame.width;
    desc.Height = frame.height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = format;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> texture;
    CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
          D3D12_RESOURCE_STATE_UNORDERED_ACCESS, nullptr, IID_PPV_ARGS(&texture)));
    D3D12_DESCRIPTOR_HEAP_DESC hd{};
    hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    hd.NumDescriptors = 1;
    ComPtr<ID3D12DescriptorHeap> cpu_heap, gpu_heap;
    CHECK(frame.dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&cpu_heap)));
    hd.Flags = D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE;
    CHECK(frame.dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&gpu_heap)));
    using Handle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
    D3D12_CPU_DESCRIPTOR_HANDLE cpu{xbox_method<Handle>(cpu_heap.Get(), 9)(cpu_heap.Get())};
    D3D12_CPU_DESCRIPTOR_HANDLE dest{xbox_method<Handle>(gpu_heap.Get(), 9)(gpu_heap.Get())};
    D3D12_GPU_DESCRIPTOR_HANDLE gpu{xbox_method<Handle>(gpu_heap.Get(), 10)(gpu_heap.Get())};
    frame.dev->CreateUnorderedAccessView(texture.Get(), nullptr, nullptr, cpu);
    frame.dev->CopyDescriptorsSimple(1, dest, cpu, hd.Type);
    auto* list = frame.list.Get();
    using Bind = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    xbox_method<Bind>(list, 32)(list, 1, gpu_heap.GetAddressOf());
    using Clear = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_GPU_DESCRIPTOR_HANDLE,
                                 D3D12_CPU_DESCRIPTOR_HANDLE, ID3D12Resource*, const T*, UINT, const D3D12_RECT*);
    auto clear = xbox_method<Clear>(list, slot);
    clear(list, gpu, cpu, texture.Get(), base, 0, nullptr);
    D3D12_RESOURCE_BARRIER order{};
    order.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    order.UAV.pResource = texture.Get();
    list->ResourceBarrier(1, &order);
    constexpr D3D12_RECT rect{2, 1, 7, 4};
    clear(list, gpu, cpu, texture.Get(), inside, 1, &rect);
    D3D12_RESOURCE_BARRIER transition{};
    transition.Transition = {texture.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &transition);
    frame.copy_to_readback(texture.Get(), format);
    CHECK(list->Close());
    ID3D12CommandList* work[] = {list};
    frame.queue->ExecuteCommandLists(1, work);
    if (frame.wait_for_gpu()) return 1;

    return check_rect_readback(frame, "UAV clear", slot, rect, expected_base, expected_inside);
}

static int check_uav_clears() {
    Frame frame;
    if (frame.init(9, 6)) return 1;
    const UINT uint_base[] = {17, 93, 201, 255}, uint_rect[] = {250, 42, 3, 88};
    int result = check_uav_clear(frame, 53, DXGI_FORMAT_R8G8B8A8_UINT, uint_base, uint_rect,
                                 {17,93,201,255}, {250,42,3,88});
    if (result) return result;
    CHECK(frame.alloc->Reset());
    CHECK(frame.list->Reset(frame.alloc.Get(), nullptr));
    // Avoid halfway values when the driver quantizes float clears to UNORM8.
    const float float_base[] = {0.f, 0.25f, 0.4f, 1.f}, float_rect[] = {1.f, 0.f, 0.75f, 0.25f};
    return check_uav_clear(frame, 54, DXGI_FORMAT_R8G8B8A8_UNORM, float_base, float_rect,
                           {0,64,102,255}, {255,0,191,64});
}

// Xbox DCC options start in the desktop description's four bytes of tail padding.
struct CommittedDescX {
    D3D12_RESOURCE_DIMENSION dimension;
    UINT64 alignment, width;
    UINT height;
    UINT16 depth_or_array_size, mip_levels;
    DXGI_FORMAT format;
    DXGI_SAMPLE_DESC samples;
    D3D12_TEXTURE_LAYOUT layout;
    UINT flags, dcc_options;
    UINT64 reserved;
};
static_assert(offsetof(CommittedDescX, dcc_options) == 0x34 && sizeof(CommittedDescX) == 0x40);

static int check_scarlett_copy() {
    Frame frame;
    if (frame.init(9, 6) || frame.begin({1, 0, 0, 1})) return 1;
    const auto format = DXGI_FORMAT_R8G8B8A8_UNORM;
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    CommittedDescX desc{};
    desc.dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.width = frame.width;
    desc.height = frame.height;
    desc.depth_or_array_size = desc.mip_levels = desc.samples.Count = 1;
    desc.format = format;
    using Create = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_HEAP_PROPERTIES*, D3D12_HEAP_FLAGS,
        const CommittedDescX*, D3D12_RESOURCE_STATES, const D3D12_CLEAR_VALUE*, REFIID, void**);
    auto create = xbox_method<Create>(frame.dev.Get(), 76);
    const auto state = D3D12_RESOURCE_STATE_COPY_DEST;
    ComPtr<ID3D12Resource> source, intermediate;
    desc.dcc_options = 3;
    CHECK(create(frame.dev.Get(), &heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&source)));
    desc.layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x101);
    desc.flags = 0xc00005;
    desc.dcc_options = 1;
    CHECK(create(frame.dev.Get(), &heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&intermediate)));
    if (!source || !intermediate) {
        std::fputs("Slot 76 returned success with a null resource\n", stderr);
        return 1;
    }
    const auto actual = intermediate->GetDesc();
    if (actual.Width != frame.width || actual.Height != frame.height || actual.Format != format ||
        actual.Layout != D3D12_TEXTURE_LAYOUT_UNKNOWN ||
        actual.Flags != (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)) {
        std::fputs("Slot 76 returned an unexpected texture description\n", stderr);
        return 1;
    }

    desc.reserved = 1;
    ComPtr<ID3D12Resource> unsupported;
    if (create(frame.dev.Get(), &heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
               IID_PPV_ARGS(&unsupported)) != E_NOTIMPL || unsupported) {
        std::fputs("Slot 76 accepted an unknown descriptor extension\n", stderr);
        return 1;
    }

    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    auto buffer = frame.readback->GetDesc();
    ComPtr<ID3D12Resource> upload;
    CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
          D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&upload)));
    BYTE* data = nullptr;
    D3D12_RANGE no_reads{};
    CHECK(upload->Map(0, &no_reads, reinterpret_cast<void**>(&data)));
    for (UINT y = 0; y < frame.height; ++y) {
        for (UINT x = 0; x < frame.width; ++x) {
            Color color = {BYTE(x * 17), BYTE(y * 31), 93, 255};
            std::memcpy(data + y * frame.pitch + x * 4, color.data(), 4);
        }
    }
    upload->Unmap(0, nullptr);
    D3D12_TEXTURE_COPY_LOCATION src{}, mid{}, dst{}, footprint{};
    src.pResource = source.Get();
    mid.pResource = intermediate.Get();
    dst.pResource = frame.target.Get();
    footprint.pResource = upload.Get();
    footprint.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    footprint.PlacedFootprint.Footprint = {format, frame.width, frame.height, 1, frame.pitch};
    auto* list = frame.list.Get();
    using Copy = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
        UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);
    auto copy = xbox_method<Copy>(list, 85);
    copy(list, &src, 0, 0, 0, &footprint, nullptr, 0);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Transition = {source.Get(), 0, state, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    copy(list, &mid, 0, 0, 0, &src, nullptr, 0);
    barrier.Transition.pResource = intermediate.Get();
    list->ResourceBarrier(1, &barrier);
    frame.transition(D3D12_RESOURCE_STATE_RENDER_TARGET, state);
    // Move a nonuniform 5x4 rectangle to a different origin.
    const D3D12_BOX box{3, 2, 0, 8, 6, 1};
    copy(list, &dst, 1, 1, 0, &mid, &box, 0);
    frame.transition(state, D3D12_RESOURCE_STATE_RENDER_TARGET);
    if (frame.finish()) return 1;
    return check_readback(frame, "Scarlett copy", 85, [](LONG x, LONG y) -> Color {
        if (x < 1 || x >= 6 || y < 1 || y >= 5) return {255, 0, 0, 255};
        return {BYTE((x + 2) * 17), BYTE((y + 1) * 31), 93, 255};
    });
}

static int check_indirect() {
    Frame frame;
    if (frame.init(12, 6) || frame.begin({1, 0, 0, 1})) return 1;
    Mesh mesh;
    const float vertices[][4] = {{-1,-1,0,1}, {-1,3,0,1}, {3,-1,0,1}};
    const D3D12_INPUT_ELEMENT_DESC position = {"POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT};
    if (mesh.init(frame.dev.Get(), {triangle_vs, sizeof(triangle_vs)}, {triangle_ps, sizeof(triangle_ps)},
                  &position, 1, sizeof(vertices)) || mesh.bind(frame, vertices, 3, sizeof(vertices[0]))) return 1;

    D3D12_INDIRECT_ARGUMENT_DESC arg{};
    arg.Type = D3D12_INDIRECT_ARGUMENT_TYPE_DRAW;
    D3D12_COMMAND_SIGNATURE_DESC desc{sizeof(D3D12_DRAW_ARGUMENTS), 1, &arg, 0};
    ComPtr<ID3D12CommandSignature> signature;
    CHECK(frame.dev->CreateCommandSignature(&desc, nullptr, IID_PPV_ARGS(&signature)));
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_UPLOAD;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    auto buffer = frame.readback->GetDesc();
    ComPtr<ID3D12Resource> args;
    CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
          D3D12_RESOURCE_STATE_GENERIC_READ, nullptr, IID_PPV_ARGS(&args)));
    // Count=0 at byte 4; the four words at byte 16 form the real draw.
    const UINT payload[] = {1, 0, 0, 0, 3, 1, 0, 0};
    void* data = nullptr;
    D3D12_RANGE no_reads{};
    CHECK(args->Map(0, &no_reads, &data));
    std::memcpy(data, payload, sizeof(payload));
    args->Unmap(0, nullptr);

    LONG x = 0;
    for (auto name : {L"scarlett_commands_legacy.dll", L"scarlett_commands_260402.dll", L"scarlett_commands_261000.dll"}) {
        HMODULE dll = LoadLibraryW(name);
        using Record = void (*)(ID3D12GraphicsCommandList*, ID3D12CommandSignature*, ID3D12Resource*, ID3D12Resource*);
        auto record = dll ? reinterpret_cast<Record>(GetProcAddress(dll, "record_draw")) : nullptr;
        if (!record) {
            std::fwprintf(stderr, L"Failed to load record_draw from %ls\n", name);
            if (dll) FreeLibrary(dll);
            return 1;
        }
        // Each caller paints a different strip. The bottom count-buffer draw must do nothing.
        D3D12_RECT strip{x, 0, x + 4, 3};
        frame.list->RSSetScissorRects(1, &strip);
        record(frame.list.Get(), signature.Get(), args.Get(), nullptr);
        strip.top = 3;
        strip.bottom = 6;
        frame.list->RSSetScissorRects(1, &strip);
        record(frame.list.Get(), signature.Get(), args.Get(), args.Get());
        FreeLibrary(dll);
        x += 4;
    }
    if (frame.finish()) return 1;
    return check_rect_readback(frame, "Indirect draw across three caller modules", 61,
                               {0,0,12,3}, {255,0,0,255}, {0,255,0,255});
}

static int check_discard() {
    Frame frame;
    if (frame.init(9, 6)) return 1;
    auto* list = frame.list.Get();
    using Discard = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12Resource*, const D3D12_DISCARD_REGION*);
    auto discard = xbox_method<Discard>(list, 55);
    discard(list, frame.target.Get(), nullptr);
    if (frame.begin({1, 0, 0, 1})) return 1;

    constexpr D3D12_RECT rect{2, 1, 7, 4};
    const D3D12_DISCARD_REGION region{1, &rect, 0, 1};
    discard(list, frame.target.Get(), &region);
    // Discarded data is undefined. Overwrite exactly the region before reading.
    using Clear = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                                 const float*, UINT, const D3D12_RECT*);
    const float green[] = {0, 1, 0, 1};
    xbox_method<Clear>(list, 52)(list, frame.rtv, green, 1, &rect);
    if (frame.finish()) return 1;
    return check_rect_readback(frame, "Discard", 55, rect, {255,0,0,255}, {0,255,0,255});
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
    if (argc == 2 && !std::strcmp(argv[1], "--thread-dispatch-test")) return check_compute(ComputeProbe::ThreadDispatch);
    if (argc == 2 && !std::strcmp(argv[1], "--compute-barrier-test")) return check_compute(ComputeProbe::UavBarrier);
    if (argc == 2 && !std::strcmp(argv[1], "--pso-fallback-test")) return check_compute(ComputeProbe::OfflineFallback);
    if (argc == 2 && !std::strcmp(argv[1], "--indirect-test")) return check_indirect();
    if (argc == 2 && !std::strcmp(argv[1], "--scarlett-copy-test")) return check_scarlett_copy();
    if (argc == 2 && !std::strcmp(argv[1], "--discard-test")) return check_discard();
    if (argc == 2 && !std::strcmp(argv[1], "--uav-test")) return check_uav_clears();
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
    const bool audio_test = argc == 2 && !std::strcmp(argv[1], "--audio-test");
    const bool headless = audio_test || placed || (argc == 2 && !std::strcmp(argv[1], "--test"));
    const bool present_test = argc == 2 && !std::strcmp(argv[1], "--present-test");
    if (argc != 1 && !headless && !present_test && !cpu_tiled && !reference && !state_test && !stream_test && !scene_3d) {
        std::fputs("usage: coin_window.exe [--thread-dispatch-test | --compute-barrier-test | --pso-fallback-test | --indirect-test | --scarlett-copy-test | --discard-test | --uav-test | --audio-test | --test | --placed-test | --present-test | --tiled | --tiled-test | --tiled-reference FILE | --mip-reference FILE | --bc1-reference FILE | --rect-{rgba,bc1}-{wide,tall} FILE | --array-{rgba-tall,bc1-wide} FILE | --cube-test FILE | --state-test | --stream-test FILE | --3d | --3d-test | --controller-test | --gameinput-test]\n", stderr);
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
    const bool scripted = headless || present_test || tiled_test || scene_test;
    CoinAudio audio;
    bool sound = audio_test || (!scripted && !controller_test);
    if (sound) {
        const HRESULT hr = audio.init();
        if (FAILED(hr)) {
            std::fprintf(stderr, "Pickup audio initialization failed (HRESULT 0x%08lx)\n", hr);
            if (audio_test) return 1;
            sound = false;
        }
    }
    // Return -1 on failure, 0 for a blocked action and 1 for a board change.
    auto move = [&](unsigned char key, CoinController* pad = nullptr) {
        const auto previous_coins = board.coins;
        bool changed = coin_move(game.get(), key);
        if ((scripted || changed) && paint()) return -1;
        if (board.coins < previous_coins) {
            if (pad) pad->pickup();
            if (sound) {
                const size_t column = board.player % board.width;
                const auto side = column < board.width / 2 ? CoinSide::Left : CoinSide::Right;
                const HRESULT hr = audio.pickup(side);
                if (FAILED(hr)) {
                    std::fprintf(stderr, "Pickup audio submission failed (HRESULT 0x%08lx)\n", hr);
                    if (audio_test) return -1;
                    sound = false;
                }
            }
        }
        return int(changed);
    };
    if (scripted) {
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
            int changed = move(step.key);
            if (changed < 0) return 1;
            if (audio_test) Sleep(200); // Separate moves and let the last tone finish before teardown.
            unsigned coins = (step.mask & 1) + (step.mask >> 1);
            if (changed != step.changed || board.player != step.player || board.coins != coins ||
                board.tiles[3] != (step.mask & 1 ? '*' : '.') ||
                board.tiles[8] != (step.mask & 2 ? '*' : '.')) {
                std::fprintf(stderr, "Game state mismatch at frame %llu after %c\n", renderer.frame.frame, step.key);
                return 1;
            }
        }
        if (audio_test) {
            // Exercise simultaneous voices separately from the four gameplay pickups.
            CHECK(audio.pickup(CoinSide::Left));
            CHECK(audio.pickup(CoinSide::Right));
            Sleep(250);
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
                    if (move(action, &pad) < 0) return 1;
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
                if (move(static_cast<unsigned char>(msg.wParam)) < 0) return 1;
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
