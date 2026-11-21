// Headless synthetic title. Xbox slot numbers follow TuxBox's device_wrap,
// cmdlist_wrap and heap_wrap. All other calls use the public Windows SDK.
#include "graphics.h"
#include "triangle_shaders.h"
#include <cmath>
#include <fstream>
#include <vector>
#include <iterator>

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

// Six vertices per cell keep interpolation constant while neighboring pixels take
// different retail shader branches. Output is tightly packed RGBA8.
static int cache_grid(const char* pixel_path, const char* attributes_path, const char* result_path, bool dual = false) {
    constexpr UINT extent = 16;
    struct Vertex { float position[4], attributes[12]; };
    std::ifstream input(pixel_path, std::ios::binary);
    std::vector<char> pixel{std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
    std::ifstream attributes(attributes_path, std::ios::binary);
    std::vector<Vertex> vertices;
    for (UINT y = 0; y < extent; ++y) {
        for (UINT x = 0; x < extent; ++x) {
            Vertex v{};
            if (!attributes.read(reinterpret_cast<char*>(v.attributes), (dual ? 12 : 8) * sizeof(float))) return 1;
            const UINT corners[][2] = {{0,0}, {1,0}, {0,1}, {0,1}, {1,0}, {1,1}};
            for (const auto& corner : corners) {
                v.position[0] = 2.0f * (x + corner[0]) / extent - 1;
                v.position[1] = 1 - 2.0f * (y + corner[1]) / extent;
                v.position[3] = 1;
                vertices.push_back(v);
            }
        }
    }
    if (pixel.empty() || input.bad() || attributes.peek() != EOF) return 1;
    const D3D12_INPUT_ELEMENT_DESC layout[] = {
        {"POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 0, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"COLOR", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 16, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"TEXCOORD", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 32, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"NORMAL", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 48, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
    };
    Frame frame;
    Mesh mesh;
    const auto vs = dual ? D3D12_SHADER_BYTECODE{cache_mrt_vs, sizeof(cache_mrt_vs)}
                         : D3D12_SHADER_BYTECODE{cache_grid_vs, sizeof(cache_grid_vs)};
    constexpr UINT plane_size = extent * D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
    if (frame.init(extent, extent, false, (dual ? 2 : 1) * plane_size) || frame.begin({1, 0, 1, 1}) ||
        mesh.init(frame.dev.Get(), vs, {pixel.data(), pixel.size()}, layout, dual ? 4 : 3,
                  UINT(vertices.size() * sizeof(Vertex)), nullptr, 0, false, {}, dual) ||
        mesh.bind(frame, vertices.data(), UINT(vertices.size()), sizeof(Vertex))) return 1;
    ComPtr<ID3D12Resource> second;
    ComPtr<ID3D12DescriptorHeap> second_view;
    if (dual) {
        auto desc = frame.target->GetDesc();
        D3D12_HEAP_PROPERTIES heap{};
        heap.Type = D3D12_HEAP_TYPE_DEFAULT;
        heap.CreationNodeMask = heap.VisibleNodeMask = 1;
        CHECK(frame.dev->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET,
                                                  nullptr, IID_PPV_ARGS(&second)));
        D3D12_DESCRIPTOR_HEAP_DESC view{D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1};
        CHECK(frame.dev->CreateDescriptorHeap(&view, IID_PPV_ARGS(&second_view)));
        using Handle = SIZE_T (WINAPI*)(ID3D12DescriptorHeap*);
        D3D12_CPU_DESCRIPTOR_HANDLE targets[] = {frame.rtv, {xbox_method<Handle>(second_view.Get(), 9)(second_view.Get())}};
        frame.dev->CreateRenderTargetView(second.Get(), nullptr, targets[1]);
        using Bind = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_CPU_DESCRIPTOR_HANDLE*, BOOL,
                                    const D3D12_CPU_DESCRIPTOR_HANDLE*);
        xbox_method<Bind>(frame.list.Get(), 50)(frame.list.Get(), 2, targets, FALSE, nullptr);
    }
    frame.list->DrawInstanced(UINT(vertices.size()), 1, 0, 0);
    if (dual) {
        D3D12_RESOURCE_BARRIER barrier{};
        barrier.Transition = {second.Get(), 0, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE};
        frame.list->ResourceBarrier(1, &barrier);
        frame.copy_to_readback(second.Get(), DXGI_FORMAT_R8G8B8A8_UNORM, plane_size);
    }
    if (frame.finish()) return 1;
    D3D12_RANGE range{0, (dual ? plane_size : 0) + (extent - 1) * frame.pitch + 4 * extent};
    void* mapped = nullptr;
    CHECK(frame.readback->Map(0, &range, &mapped));
    std::ofstream output(result_path, std::ios::binary);
    for (UINT plane = 0; plane < (dual ? 2u : 1u); ++plane)
        for (UINT y = 0; y < extent; ++y)
            output.write(static_cast<const char*>(mapped) + plane_size * plane + frame.pitch * y, 4 * extent);
    D3D12_RANGE no_writes{};
    frame.readback->Unmap(0, &no_writes);
    output.close();
    return output.good() ? 0 : 1;
}

// Two quads covering disjoint halves of the viewport, with opposite winding.
static int cache_facing(const char* pixel_path, const char* result_path) {
    std::ifstream input(pixel_path, std::ios::binary);
    std::vector<char> pixel((std::istreambuf_iterator<char>(input)), {});
    if (pixel.empty()) return 1;
    constexpr UINT extent = 16;
    const float verts[][4] = {
        {-1,1,0,1},{0,1,0,1},{0,-1,0,1}, {-1,1,0,1},{0,-1,0,1},{-1,-1,0,1},
        {0,1,0,1},{1,-1,0,1},{1,1,0,1}, {0,1,0,1},{0,-1,0,1},{1,-1,0,1}
    };
    Frame frame;
    Mesh mesh;
    const D3D12_INPUT_ELEMENT_DESC layout{"POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT};
    if (frame.init(extent, extent) || frame.begin({1,0,1,1}) ||
        mesh.init(frame.dev.Get(), {triangle_vs,sizeof(triangle_vs)}, {pixel.data(),pixel.size()},
                  &layout,1,sizeof(verts)) || mesh.draw(frame,verts,12,sizeof(verts[0])) || frame.finish()) return 1;
    void* data;
    CHECK(frame.readback->Map(0,nullptr,&data));
    std::ofstream out(result_path,std::ios::binary);
    for (UINT y = 0; y < extent; ++y) out.write(static_cast<char*>(data)+y*frame.pitch,extent*4);
    out.close();
    frame.readback->Unmap(0,nullptr);
    return !out;
}

int main(int argc, char** argv) {
    if (argc == 4 && !std::strcmp(argv[1], "--cache-facing")) return cache_facing(argv[2], argv[3]);
    if (argc == 5 && std::strcmp(argv[1], "--cache-grid") == 0) return cache_grid(argv[2], argv[3], argv[4]);
    if (argc == 5 && std::strcmp(argv[1], "--cache-mrt-grid") == 0) return cache_grid(argv[2], argv[3], argv[4], true);
    const bool cache_pixel = argc == 4 && std::strcmp(argv[1], "--cache-pixel") == 0;
    const bool draw_triangle = cache_pixel || (argc == 2 && std::strcmp(argv[1], "--triangle") == 0);
    if (argc != 1 && !draw_triangle) {
        std::fputs("usage: clear_pixel.exe [--triangle | --cache-pixel FILE RESULT | --cache-facing PS RESULT | --cache-grid PS ATTRIBUTES RESULT | --cache-mrt-grid PS ATTRIBUTES RESULT]\n", stderr);
        return 1;
    }
    const UINT extent = draw_triangle ? 16 : 1;
    if (check_root_signature()) {
        std::fputs("Root signature round trip failed\n", stderr);
        return 1;
    }
    Frame frame;
    if (frame.init(extent, extent) || frame.begin({1, 0, 1, 1})) return 1;
    Mesh mesh;
    if (draw_triangle) {
        const float positions[][4] = {{-.75f, -.75f, 0, 1}, {0, .75f, 0, 1}, {.75f, -.75f, 0, 1}};
        const D3D12_INPUT_ELEMENT_DESC position = {
            "POSITION", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 0, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0
        };
        D3D12_SHADER_BYTECODE pixel{triangle_ps, sizeof(triangle_ps)};
        std::vector<char> cache_blob;
        if (cache_pixel) {
            std::ifstream input(argv[2], std::ios::binary);
            cache_blob.assign(std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>());
            if (input.bad() || cache_blob.empty()) return 1;
            pixel = {cache_blob.data(), cache_blob.size()};
        }
        if (mesh.init(frame.dev.Get(), {triangle_vs, sizeof(triangle_vs)}, pixel,
                      &position, 1, sizeof(positions)) || mesh.draw(frame, positions, 3, sizeof(positions[0])))
            return 1;
    }
    if (frame.finish()) return 1;
    D3D12_RANGE range = {0, (extent - 1) * frame.pitch + extent * 4};
    void* mapped = nullptr;
    CHECK(frame.readback->Map(0, &range, &mapped));
    const auto* bytes = static_cast<const unsigned char*>(mapped);
    auto pixel_matches = [&](UINT x, UINT y, const unsigned char (&expected)[4]) {
        const auto* pixel = bytes + y * frame.pitch + 4 * x;
        if (draw_triangle) std::printf("(%u,%u) ", x, y);
        std::printf("RGBA readback: %u %u %u %u\n", pixel[0], pixel[1], pixel[2], pixel[3]);
        return std::memcmp(pixel, expected, 4) == 0;
    };
    const unsigned char background[] = {255, 0, 255, 255};
    // Cache shader 4553 exports half pairs (0,1) and (0,0).
    const unsigned char green[] = {0, 255, 0, static_cast<unsigned char>(cache_pixel ? 0 : 255)};
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
                if (std::memcmp(bytes + y * frame.pitch + x * 4, inside ? green : background, 4) != 0)
                    ++mismatches;
            }
        }
        std::printf("Triangle coverage: %u/%u pixels match\n", extent * extent - mismatches, extent * extent);
        pass = pass && mismatches == 0;
    }
    D3D12_RANGE no_writes = {};
    frame.readback->Unmap(0, &no_writes);
    if (cache_pixel && pass) {
        std::ofstream result(argv[3]);
        result << extent * extent << '\n';
        result.close();
        pass = result.good();
    }
    return pass ? 0 : 1;
}
