// Run one unmodified wave64 PS09 program with its original scalar/table ABI.
#include "hsa_dispatch.h"
#include "../ps09_raster_inputs.h"
#include <hsa/hsa_ext_image.h>
#include <sys/mman.h>

using namespace raster_fixture;

struct Storage {
    Agents agents;
    std::vector<void*> allocations;

    int allocate(size_t size, void** out) {
        HSA(hsa_amd_memory_pool_allocate(agents.pool, size, 0, out));
        HSA(hsa_amd_agents_allow_access(1, &agents.gpu, nullptr, *out));
        allocations.push_back(*out);
        std::memset(*out, 0, size);
        return 0;
    }

    int image(bool target, void* pixels, hsa_ext_image_t* object) {
        hsa_ext_image_descriptor_t desc{};
        desc.geometry = HSA_EXT_IMAGE_GEOMETRY_2D;
        desc.width = target ? 2 : 16;
        desc.height = target ? 2 : 1;
        desc.format.channel_order = target ? HSA_EXT_IMAGE_CHANNEL_ORDER_RG : HSA_EXT_IMAGE_CHANNEL_ORDER_R;
        desc.format.channel_type = target ? HSA_EXT_IMAGE_CHANNEL_TYPE_UNSIGNED_INT32 : HSA_EXT_IMAGE_CHANNEL_TYPE_FLOAT;
        auto access = target ? HSA_ACCESS_PERMISSION_RW : HSA_ACCESS_PERMISSION_RO;
        hsa_ext_image_data_info_t info{};
        HSA(hsa_ext_image_data_get_info(agents.gpu, &desc, access, &info));
        void* backing;
        if (allocate(info.size, &backing)) return 1;
        if (uintptr_t(backing) % info.alignment != 0) return 1;
        HSA(hsa_ext_image_create(agents.gpu, &desc, backing, access, object));
        hsa_ext_image_region_t region{{0, 0, 0}, {uint32_t(desc.width), uint32_t(desc.height), 1}};
        HSA(hsa_ext_image_import(agents.gpu, pixels, pitch, 0, *object, &region));
        return 0;
    }
};

int main(int argc, char** argv) {
    if (argc != 5 || std::strlen(argv[3]) != 1 || argv[3][0] < '0' || argv[3][0] > '2') return 1;
    Storage storage{{argv[1]}, {}};
    auto& agents = storage.agents;
    HSA(hsa_init());
    HSA(hsa_iterate_agents(find_agent, &agents));
    if (!agents.gpu.handle || !agents.pool.handle) {
        std::fprintf(stderr, "Missing %s GPU or CPU kernarg pool\n", agents.name);
        return 1;
    }
    std::ifstream code(argv[2], std::ios::binary);
    std::vector<char> object{std::istreambuf_iterator<char>(code), {}};
    if (object.empty() || code.bad()) return 1;

    // The guest explicitly zeros the high halves of both table addresses.
    auto tables = mmap(nullptr, 8192, PROT_READ | PROT_WRITE, MAP_32BIT | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (tables == MAP_FAILED) return 1;
    void* device_tables;
    HSA(hsa_amd_memory_lock(tables, 8192, &agents.gpu, 1, &device_tables));
    if (device_tables != tables || uintptr_t(device_tables) > UINT32_MAX - 8191) return 1;
    auto table0 = static_cast<uint32_t*>(tables);
    auto table1 = table0 + 1024;
    void* allocation;
    if (storage.allocate(texture_offset + 1024, &allocation)) return 1;
    auto bytes = static_cast<unsigned char*>(allocation);
    initialize(bytes, argv[3][0] - '0');
    auto descriptor = [&](uint32_t* out, unsigned section, unsigned stride) {
        const uintptr_t addr = uintptr_t(bytes + section * block);
        out[0] = uint32_t(addr);
        out[1] = uint32_t(addr >> 32) | (stride << 16);
        out[2] = block / (stride ? stride : 1);
        out[3] = 0x11014fac;  // 32-bit raw S#, identity selectors
    };
    descriptor(table0 + 104, 0, 2048);
    descriptor(table0 + 120, 1, 1);
    descriptor(table0 + 144, 2, 176);
    descriptor(table0 + 168, 3, 336);
    descriptor(table1 + 8, 4, 0);
    descriptor(table1 + 16, 5, 0);
    hsa_ext_image_t input{}, output{};
    if (storage.image(false, bytes + texture_offset, &input)
        || storage.image(true, bytes + texture_offset + 512, &output)) return 1;
    // On GFX10, the HSA image handle points at the hardware image descriptor.
    std::memcpy(table0 + 232, reinterpret_cast<void*>(input.handle), 32);
    std::memcpy(table1, reinterpret_cast<void*>(output.handle), 32);
    void* args;
    if (storage.allocate(8, &args)) return 1;
    uint32_t addresses[] = {uint32_t(uintptr_t(table0)), uint32_t(uintptr_t(table1))};
    std::memcpy(args, addresses, sizeof(addresses));
    if (dispatch(agents, object, "probe.kd", 64, args)) return 1;
    hsa_ext_image_region_t region{{0, 0, 0}, {2, 2, 1}};
    void* readback;
    if (storage.allocate(32, &readback)) return 1;
    HSA(hsa_ext_image_export(agents.gpu, output, readback, 0, 0, &region));
    std::ofstream file(argv[4], std::ios::binary);
    file.write(static_cast<char*>(readback), 32);
    file.close();
    HSA(hsa_ext_image_destroy(agents.gpu, output));
    HSA(hsa_ext_image_destroy(agents.gpu, input));
    for (void* p : storage.allocations) { HSA(hsa_amd_memory_pool_free(p)); }
    HSA(hsa_amd_memory_unlock(tables));
    munmap(tables, 8192);
    HSA(hsa_shut_down());
    return file.good() ? 0 : 1;
}
