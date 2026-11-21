#pragma once
#include <hsa/hsa.h>
#include <hsa/hsa_ext_amd.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>

#define HSA(call) do { auto status = (call); if (status != HSA_STATUS_SUCCESS) { \
    const char* why = nullptr; hsa_status_string(status, &why); \
    std::fprintf(stderr, "%s failed: %s\n", #call, why ? why : "unknown status"); return 1; \
} } while (0)

struct Agents {
    const char* name;
    hsa_agent_t gpu{};
    hsa_amd_memory_pool_t pool{};
};

static hsa_status_t find_pool(hsa_amd_memory_pool_t pool, void* context) {
    uint32_t flags = 0;
    auto status = hsa_amd_memory_pool_get_info(pool, HSA_AMD_MEMORY_POOL_INFO_GLOBAL_FLAGS, &flags);
    if (status == HSA_STATUS_SUCCESS && (flags & HSA_AMD_MEMORY_POOL_GLOBAL_FLAG_KERNARG_INIT))
        static_cast<Agents*>(context)->pool = pool;
    return status;
}

static hsa_status_t find_agent(hsa_agent_t agent, void* context) {
    auto& agents = *static_cast<Agents*>(context);
    hsa_device_type_t type;
    auto status = hsa_agent_get_info(agent, HSA_AGENT_INFO_DEVICE, &type);
    if (status != HSA_STATUS_SUCCESS) return status;
    if (type == HSA_DEVICE_TYPE_CPU)
        return hsa_amd_agent_iterate_memory_pools(agent, find_pool, context);
    if (type == HSA_DEVICE_TYPE_GPU) {
        char name[64]{};
        status = hsa_agent_get_info(agent, HSA_AGENT_INFO_NAME, name);
        if (status == HSA_STATUS_SUCCESS && std::strcmp(name, agents.name) == 0) agents.gpu = agent;
    }
    return status;
}

// Launch a single workgroup with a caller-supplied kernarg block.
static int dispatch(const Agents& agents, const std::vector<char>& object,
                    const char* symbol_name, uint16_t lanes, void* args) {
    hsa_code_object_reader_t reader;
    HSA(hsa_code_object_reader_create_from_memory(object.data(), object.size(), &reader));
    hsa_executable_t exe;
    HSA(hsa_executable_create_alt(HSA_PROFILE_FULL, HSA_DEFAULT_FLOAT_ROUNDING_MODE_DEFAULT, nullptr, &exe));
    HSA(hsa_executable_load_agent_code_object(exe, agents.gpu, reader, nullptr, nullptr));
    HSA(hsa_executable_freeze(exe, nullptr));
    hsa_executable_symbol_t symbol;
    HSA(hsa_executable_get_symbol_by_name(exe, symbol_name, &agents.gpu, &symbol));
    hsa_queue_t* queue;
    HSA(hsa_queue_create(agents.gpu, 128, HSA_QUEUE_TYPE_MULTI, nullptr, nullptr, UINT32_MAX, UINT32_MAX, &queue));
    hsa_signal_t signal;
    HSA(hsa_signal_create(1, 0, nullptr, &signal));
    auto idx = hsa_queue_add_write_index_relaxed(queue, 1);
    auto& packet = static_cast<hsa_kernel_dispatch_packet_t*>(queue->base_address)[idx & (queue->size-1)];
    packet.setup = 1u << HSA_KERNEL_DISPATCH_PACKET_SETUP_DIMENSIONS;
    packet.workgroup_size_x = lanes;
    packet.workgroup_size_y = packet.workgroup_size_z = 1;
    packet.grid_size_x = lanes;
    packet.grid_size_y = packet.grid_size_z = 1;
    packet.kernarg_address = args;
    packet.completion_signal = signal;
    HSA(hsa_executable_symbol_get_info(symbol, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_OBJECT, &packet.kernel_object));
    HSA(hsa_executable_symbol_get_info(symbol, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_GROUP_SEGMENT_SIZE, &packet.group_segment_size));
    HSA(hsa_executable_symbol_get_info(symbol, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_PRIVATE_SEGMENT_SIZE, &packet.private_segment_size));
    uint16_t header = HSA_PACKET_TYPE_KERNEL_DISPATCH | (HSA_FENCE_SCOPE_SYSTEM << HSA_PACKET_HEADER_SCACQUIRE_FENCE_SCOPE)
                    | (HSA_FENCE_SCOPE_SYSTEM << HSA_PACKET_HEADER_SCRELEASE_FENCE_SCOPE);
    __atomic_store_n(&packet.header, header, __ATOMIC_RELEASE);
    hsa_signal_store_screlease(queue->doorbell_signal, idx);
    while (hsa_signal_wait_scacquire(signal, HSA_SIGNAL_CONDITION_EQ, 0, UINT64_MAX, HSA_WAIT_STATE_BLOCKED) != 0) {
        // ROCr permits spurious wakeups before the dispatch completes.
    }
    HSA(hsa_signal_destroy(signal));
    HSA(hsa_queue_destroy(queue));
    HSA(hsa_executable_destroy(exe));
    HSA(hsa_code_object_reader_destroy(reader));
    return 0;
}
