// Dispatch one single-work-item, ordinary ROCr code object. The buffer contains
// the same register inputs sent to Coin Run followed by space for native masks.
#include "hsa_dispatch.h"

int main(int argc, char** argv) {
    if (argc != 5) return 1;
    Agents agents{argv[1]};
    std::ifstream state(argv[3], std::ios::binary), code(argv[2], std::ios::binary);
    std::vector<char> input{std::istreambuf_iterator<char>(state), {}};
    std::vector<char> object{std::istreambuf_iterator<char>(code), {}};
    if (input.empty() || object.empty() || state.bad() || code.bad()) return 1;
    HSA(hsa_init());
    HSA(hsa_iterate_agents(find_agent, &agents));
    if (!agents.gpu.handle || !agents.pool.handle) {
        std::fprintf(stderr, "Missing %s GPU or CPU kernarg pool\n", agents.name);
        return 1;
    }
    void *args = nullptr, *buffer = nullptr;
    HSA(hsa_amd_memory_pool_allocate(agents.pool, sizeof(buffer), 0, &args));
    HSA(hsa_amd_memory_pool_allocate(agents.pool, input.size(), 0, &buffer));
    HSA(hsa_amd_agents_allow_access(1, &agents.gpu, nullptr, args));
    HSA(hsa_amd_agents_allow_access(1, &agents.gpu, nullptr, buffer));
    std::memcpy(args, &buffer, sizeof(buffer));
    std::memcpy(buffer, input.data(), input.size());
    if (dispatch(agents, object, "compare.kd", 1, args)) return 1;
    std::ofstream output(argv[4], std::ios::binary);
    output.write(static_cast<const char*>(buffer), input.size());
    output.close();
    HSA(hsa_amd_memory_pool_free(buffer));
    HSA(hsa_amd_memory_pool_free(args));
    HSA(hsa_shut_down());
    return output.good() ? 0 : 1;
}
