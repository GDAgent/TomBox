// An ordinal-importing consumer. The Linux fixture drives hotplug while the
// main thread waits for notifications without polling GetCurrentReading.
#include <windows.h>
#include <GameInput.h>
#include <wrl/client.h>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <mutex>

using Microsoft::WRL::ComPtr;
static constexpr auto online = GameInputDeviceConnected | GameInputDeviceInputEnabled;

static GameInputDeviceStatus online_status(IGameInputDevice* device) {
    return device->GetDeviceInfo()->supportedRumbleMotors ? online | GameInputDeviceOutputEnabled : online;
}

struct Watch {
    IGameInput* input;
    GameInputCallbackToken token = 0;
    std::mutex mutex;
    std::condition_variable changed;
    unsigned count = 0;
    bool valid = true, pause = false, resume = false, stop_self = false;
    ComPtr<IGameInputDevice> first_device, device;
    uint64_t timestamp = 0;
    GameInputDeviceStatus status{};

    explicit Watch(IGameInput* source) : input(source) {}
    ~Watch() {
        release_callback();
        if (token) input->UnregisterCallback(token, UINT64_MAX);
    }

    static void CALLBACK notify(GameInputCallbackToken token, void* context, IGameInputDevice* device,
                                uint64_t time, GameInputDeviceStatus current, GameInputDeviceStatus previous) {
        auto& self = *static_cast<Watch*>(context);
        std::unique_lock<std::mutex> lock(self.mutex);
        self.valid &= token == self.token && time >= self.timestamp && time <= self.input->GetCurrentTimestamp();
        const auto connected = online_status(device);
        self.valid &= (current == connected && previous == GameInputDeviceNoStatus) ||
                      (previous == connected && current == GameInputDeviceNoStatus);
        if (current == connected) {
            // The callback must be able to reenter the runtime and read this same pad.
            ComPtr<IGameInputReading> reading;
            ComPtr<IGameInputDevice> from_reading;
            if (SUCCEEDED(self.input->GetCurrentReading(GameInputKindGamepad, device, &reading)) && reading)
                reading->GetDevice(&from_reading);
            self.valid &= from_reading.Get() == device;
        }
        if (self.stop_self) self.input->StopCallback(GAMEINPUT_CURRENT_CALLBACK_TOKEN_VALUE);
        if (!self.count) self.first_device = device;
        self.device = device;
        self.timestamp = time;
        self.status = current;
        ++self.count;
        self.changed.notify_all();
        if (self.pause)
            self.valid &= self.changed.wait_for(lock, std::chrono::seconds(8), [&] { return self.resume; });
    }

    bool start(GameInputEnumerationKind mode, IGameInputDevice* filter = nullptr,
               GameInputKind kinds = GameInputKindGamepad, GameInputDeviceStatus statuses = GameInputDeviceConnected) {
        return SUCCEEDED(input->RegisterDeviceCallback(filter, kinds, statuses, mode, this, notify, &token)) && token;
    }
    void release_callback() {
        std::lock_guard<std::mutex> lock(mutex);
        resume = true;
        changed.notify_all();
    }
    bool has(unsigned expected_count, GameInputDeviceStatus expected_status, bool wait = false) {
        std::unique_lock<std::mutex> lock(mutex);
        if (wait) changed.wait_for(lock, std::chrono::seconds(8), [&] { return count >= expected_count; });
        if (expected_status == online && device) expected_status = online_status(device.Get());
        return valid && count == expected_count && status == expected_status;
    }
};

static void signal(const char* text) {
    std::puts(text);
    std::fflush(stdout);
}

static int check(IGameInput* input) {
    Watch live(input), replay(input), specific(input), stopped(input), busy(input), keyboard(input), audio(input);
    if (!live.start(GameInputNoEnumeration)) return 1;
    signal("pad watching");
    if (!live.has(1, online, true)) return 2;
    auto first = live.device;
    if (!replay.start(GameInputBlockingEnumeration) || !replay.has(1, online) || replay.device != first) return 3;
    if (!specific.start(GameInputNoEnumeration, first.Get(), GameInputKindGamepad, GameInputDeviceInputEnabled) ||
        !specific.has(0, GameInputDeviceNoStatus)) return 4;
    stopped.stop_self = true;
    if (!stopped.start(GameInputAsyncEnumeration) || !stopped.has(1, online, true)) return 5;
    busy.pause = true;
    if (!busy.start(GameInputAsyncEnumeration) || !busy.has(1, online, true)) return 6;
    if (input->UnregisterCallback(busy.token, 2000)) return 7;
    busy.release_callback();
    if (!input->UnregisterCallback(busy.token, 2000000)) return 8;
    busy.token = 0;
    if (!keyboard.start(GameInputBlockingEnumeration, nullptr, GameInputKindKeyboard) ||
        !audio.start(GameInputBlockingEnumeration, nullptr, GameInputKindGamepad, GameInputDeviceAudioRender)) return 9;
    signal("pad enumerated");
    if (!live.has(2, online, true) || live.device == first) return 10;
    auto second = live.device;
    {
        Watch pair(input);
        if (!pair.start(GameInputBlockingEnumeration) || !pair.has(2, online) ||
            !((pair.first_device == first && pair.device == second) ||
              (pair.first_device == second && pair.device == first)) ||
            !specific.has(0, GameInputDeviceNoStatus) || !stopped.has(1, online)) return 11;
    }
    signal("pad pair verified");
    if (!live.has(3, GameInputDeviceNoStatus, true) || live.device != second ||
        !specific.has(0, GameInputDeviceNoStatus)) return 12;
    signal("pad second detached");
    if (!live.has(4, GameInputDeviceNoStatus, true) || !specific.has(1, GameInputDeviceNoStatus, true) ||
        !replay.has(4, GameInputDeviceNoStatus, true) || specific.device != first) return 13;
    if (!input->UnregisterCallback(replay.token, UINT64_MAX)) return 14;
    replay.token = 0;
    signal("pad callback detached");
    if (!live.has(5, online, true) || live.device == first || live.device == second) return 15;
    // A blocking registration is a queue barrier after the replacement notification.
    Watch barrier(input);
    if (!barrier.start(GameInputBlockingEnumeration) || !barrier.has(1, online) ||
        barrier.device != live.device || !replay.has(4, GameInputDeviceNoStatus) ||
        !specific.has(1, GameInputDeviceNoStatus) || !stopped.has(1, online) ||
        !busy.has(1, online) || !keyboard.has(0, GameInputDeviceNoStatus) ||
        !audio.has(0, GameInputDeviceNoStatus)) return 16;
    return 0;
}

int main() {
    ComPtr<IGameInput> input;
    if (FAILED(GameInputCreate(&input)) || !input) return 1;
    int status = check(input.Get());
    if (status) std::fprintf(stderr, "Device callback check %d failed\n", status);
    else signal("pad callbacks verified");
    return status;
}
