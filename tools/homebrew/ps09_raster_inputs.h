#pragma once
#include <cstdint>
#include <cstring>
#include <initializer_list>

// Identical material, vertices, transforms and seed pixels for the translated
// D3D12 dispatch and the independent native GFX103x dispatch.
namespace raster_fixture {
constexpr uint32_t pitch = 256, block = 4096, texture_offset = block * 6;
constexpr uint64_t winner = 0x7fff800000000001ull;
constexpr uint64_t guard0 = 0x0123456789abcdefull, guard1 = 0xfedcba9876543210ull;

inline void initialize(unsigned char* bytes, unsigned mode) {
    auto write = [&](uint32_t section, uint32_t offset, uint32_t value) {
        std::memcpy(bytes + section * block + offset, &value, 4);
    };
    // Four raw SRVs in range order. t13 is zero. t73 contains two
    // xyz16/radius records, t72 contains the identity, t71 selects one segment.
    const uint32_t endpoints[] = {0, 20000, 32767, 0x3c00, 65535, 20000, 32767, 0x3c00};
    std::memcpy(bytes + block, endpoints, sizeof(endpoints));
    constexpr uint32_t one = 0x3f800000;
    for (uint32_t i = 0; i < 3; i++) write(2, 48 + 20 * i, one);
    write(3, 0x54, mode == 2 ? 0 : 0x02020000);
    write(3, 0x104, 1);
    write(3, 0x5c, 0x3c00);
    for (uint32_t offset : {0x60u, 0x64u, 0x68u}) write(3, offset, 0x3c000000);
    write(3, 0x8c, 0xffffffffu);
    for (uint32_t i = 0; i < 3; i++) write(4, i * 4, one);   // b15 viewport bounds
    for (uint32_t i = 0; i < 4; i++) write(5, 0x160 + i * 20, one);  // b0 view-projection
    write(5, 0x40, one);
    write(5, 0x54, one);
    for (uint32_t i = 0; i < 16; i++) write(6, 4 * i, one);
    uint64_t seed[] = {mode == 1 ? winner - 1 : 0, mode == 1 ? winner + 1 : 0, guard0, guard1};
    std::memcpy(bytes + texture_offset + 512, seed, 16);
    std::memcpy(bytes + texture_offset + 512 + pitch, seed + 2, 16);
}
} // namespace raster_fixture
