// Compare package-relative Win32 reads with byte ranges sampled independently on Linux.
#include <windows.h>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>

static bool check_range(const std::string& line) {
    std::istringstream in(line);
    std::string size, offset, expected, name;
    if (!std::getline(in, size, '\t') || !std::getline(in, offset, '\t') ||
        !std::getline(in, expected, '\t') || !std::getline(in, name) ||
        expected.empty() || expected.size() % 2 || expected.size() > 128) return false;
    auto path = std::filesystem::u8path(name);
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                              OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    LARGE_INTEGER actual_size{}, pos{};
    pos.QuadPart = std::stoll(offset);
    unsigned char data[64]{};
    DWORD length = static_cast<DWORD>(expected.size() / 2), received = 0;
    bool ok = GetFileSizeEx(file, &actual_size) && actual_size.QuadPart == std::stoll(size) &&
              SetFilePointerEx(file, pos, nullptr, FILE_BEGIN) &&
              ReadFile(file, data, length, &received, nullptr) && received == length;
    CloseHandle(file);
    constexpr char hex[] = "0123456789abcdef";
    for (DWORD i = 0; ok && i < length; ++i)
        ok = expected[2*i] == hex[data[i] >> 4] && expected[2*i+1] == hex[data[i] & 15];
    if (!ok) std::fprintf(stderr, "Read mismatch for %s at %s\n", name.c_str(), offset.c_str());
    return ok;
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 4 || !SetCurrentDirectoryW(argv[1])) return 1;
    std::ifstream in{std::filesystem::path(argv[2])};
    unsigned count = 0;
    std::string line;
    while (std::getline(in, line)) {
        if (!check_range(line)) return 1;
        ++count;
    }
    if (!in.eof() || !count) return 1;
    std::ofstream result{std::filesystem::path(argv[3])};
    result << count << '\n';
    result.close();
    return result ? 0 : 1;
}
