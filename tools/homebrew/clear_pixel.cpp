// Headless synthetic title. Xbox slot numbers follow TomBox's device_wrap,
// cmdlist_wrap and heap_wrap. All other calls use the public Windows SDK.
#include "graphics.h"
#include "triangle_shaders.h"
#include <cmath>

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

int main(int argc, char** argv) {
    const bool draw_triangle = argc == 2 && std::strcmp(argv[1], "--triangle") == 0;
    if (argc != 1 && !draw_triangle) {
        std::fputs("usage: clear_pixel.exe [--triangle]\n", stderr);
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
        if (mesh.init(frame.dev.Get(), {triangle_vs, sizeof(triangle_vs)}, {triangle_ps, sizeof(triangle_ps)},
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
                if (std::memcmp(bytes + y * frame.pitch + x * 4, inside ? green : background, 4) != 0)
                    ++mismatches;
            }
        }
        std::printf("Triangle coverage: %u/%u pixels match\n", extent * extent - mismatches, extent * extent);
        pass = pass && mismatches == 0;
    }
    D3D12_RANGE no_writes = {};
    frame.readback->Unmap(0, &no_writes);
    return pass ? 0 : 1;
}
