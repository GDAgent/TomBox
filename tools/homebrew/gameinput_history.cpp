// Exercises the game's input loop with no callbacks or reads during the tap.
#include <windows.h>
#include <GameInput.h>
#include <wrl/client.h>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdio>

using Microsoft::WRL::ComPtr;

static void signal(const char* message) {
    std::puts(message);
    std::fflush(stdout);
}

static ComPtr<IGameInputReading> await_pad(IGameInput* input, IGameInputDevice* other) {
    const auto deadline = GetTickCount64() + 10000;
    do {
        ComPtr<IGameInputReading> reading;
        if (SUCCEEDED(input->GetCurrentReading(GameInputKindGamepad, nullptr, &reading))) {
            ComPtr<IGameInputDevice> device;
            reading->GetDevice(&device);
            if (device && device.Get() != other) return reading;
        }
        Sleep(10);
    } while (GetTickCount64() < deadline);
    return {};
}

static bool matches(IGameInputReading* reading, IGameInputDevice* device, bool pressed, bool tilted) {
    ComPtr<IGameInputDevice> source;
    reading->GetDevice(&source);
    GameInputGamepadState state{};
    return source.Get() == device && reading->GetGamepadState(&state) &&
        state.buttons == (pressed ? GameInputGamepadA : GameInputGamepadNone) &&
        (tilted ? state.leftThumbstickX > 0.75f : std::abs(state.leftThumbstickX) < 0.05f);
}

static bool at_end(IGameInput* input, IGameInputReading* cursor, IGameInputDevice* device) {
    ComPtr<IGameInputReading> result;
    return input->GetNextReading(cursor, GameInputKindGamepad, device, &result) == GAMEINPUT_E_READING_NOT_FOUND && !result;
}

// The API returns an AddRef'd reading on success and clears the output on failure.
static bool previous_is(IGameInput* input, IGameInputReading* from, IGameInputDevice* filter,
                        IGameInputReading* expected, GameInputKind kind = GameInputKindGamepad) {
    ComPtr<IGameInputReading> previous;
    return input->GetPreviousReading(from, kind, filter, &previous) == S_OK && previous.Get() == expected;
}

static bool previous_fails(IGameInput* input, IGameInputReading* from, IGameInputDevice* filter,
                           HRESULT expected, GameInputKind kind = GameInputKindGamepad) {
    IGameInputReading* result = from;
    const auto hr = input->GetPreviousReading(from, kind, filter, &result);
    if (SUCCEEDED(hr) && result) result->Release();
    return hr == expected && !result;
}

// Retain callback readings so the consumer can compare their COM identities with history.
struct Subscription {
    struct Delivery {
        ComPtr<IGameInputReading> reading;
        bool overrun = false;
    };
    IGameInput* input;
    GameInputCallbackToken token = 0;
    std::array<Delivery, 96> deliveries{};
    std::atomic<unsigned> count{0};
    std::atomic<bool> blocked{false};

    explicit Subscription(IGameInput* input) : input(input) {}
    ~Subscription() {
        blocked = false;
        if (token) input->UnregisterCallback(token, UINT64_MAX);
    }
    static void CALLBACK receive(GameInputCallbackToken token, void* context, IGameInputReading* reading, bool overrun) {
        auto& self = *static_cast<Subscription*>(context);
        unsigned i = self.count.load();
        if (i < self.deliveries.size() && token == self.token)
            self.deliveries[i] = {reading, overrun};
        self.count.store(i + 1);
        const auto deadline = GetTickCount64() + 8000;
        while (self.blocked && GetTickCount64() < deadline) Sleep(2);
    }
    bool start(IGameInputDevice* filter, GameInputKind kinds, float threshold) {
        return SUCCEEDED(input->RegisterReadingCallback(filter, kinds, threshold, this, receive, &token)) && token;
    }
    bool await(unsigned n) const {
        const auto deadline = GetTickCount64() + 8000;
        while (count.load() < n && GetTickCount64() < deadline) Sleep(2);
        return count.load() == n;
    }
    bool unregister() {
        if (!input->UnregisterCallback(token, UINT64_MAX)) return false;
        token = 0;
        return true;
    }
};

static int check_subscriptions(IGameInput* input, IGameInputDevice* primary, IGameInputDevice* secondary) {
    Subscription filtered(input), all(input), keyboard(input);
    if (!filtered.start(primary, GameInputKindGamepad, 0) ||
        !all.start(nullptr, GameInputKindUnknown, 0.5f) ||
        !keyboard.start(nullptr, GameInputKindKeyboard, 0)) return 20;
    signal("pad subscriptions ready");
    if (!all.await(4) || !filtered.await(2)) return 21;
    for (unsigned i = 0; i < 4; ++i) {
        const auto& event = all.deliveries[i];
        if (event.overrun || !event.reading ||
            !matches(event.reading.Get(), i == 0 || i == 3 ? primary : secondary, i == 0, i == 1)) return 22;
    }
    if (filtered.deliveries[0].reading != all.deliveries[0].reading ||
        filtered.deliveries[1].reading != all.deliveries[3].reading ||
        filtered.deliveries[0].overrun || filtered.deliveries[1].overrun || !filtered.unregister()) return 23;
    signal("pad unsubscribed");
    if (!all.await(6)) return 24;
    auto cursor = filtered.deliveries[1].reading;
    for (unsigned i = 4; i < 6; ++i) {
        ComPtr<IGameInputReading> next;
        if (FAILED(input->GetNextReading(cursor.Get(), GameInputKindGamepad, primary, &next)) ||
            next != all.deliveries[i].reading || all.deliveries[i].overrun ||
            !matches(next.Get(), primary, i == 4, false)) return 25;
        cursor = next;
    }
    if (!at_end(input, cursor.Get(), primary) || filtered.count != 2 || keyboard.count != 0) return 26;
    return 0;
}

static int check_overrun(IGameInput* input, IGameInputDevice* primary) {
    Subscription slow(input), canceled(input);
    slow.blocked = true;
    if (!slow.start(primary, GameInputKindGamepad, 0)) return 27;
    signal("pad stall ready");
    if (!slow.await(1)) return 28;
    // A second registration accumulates work while the dispatcher is blocked.
    if (!canceled.start(primary, GameInputKindGamepad, 0)) return 32;
    signal("pad subscriber stalled");
    // While the first callback is blocked, wait until the sampler sees the final B press.
    const auto deadline = GetTickCount64() + 8000;
    ComPtr<IGameInputReading> last;
    for (;;) {
        GameInputGamepadState state{};
        if (SUCCEEDED(input->GetCurrentReading(GameInputKindGamepad, primary, &last)) &&
            last->GetGamepadState(&state) && state.buttons == GameInputGamepadB) break;
        if (GetTickCount64() >= deadline) return 29;
        Sleep(5);
    }
    if (!canceled.unregister()) return 33;
    slow.blocked = false;
    if (!slow.await(65) || !slow.unregister() || slow.count != 65 ||
        !slow.deliveries[64].overrun || slow.deliveries[64].reading != last) return 30;
    for (unsigned i = 0; i < 64; ++i)
        if (!slow.deliveries[i].reading || slow.deliveries[i].overrun) return 31;
    if (canceled.count != 0) return 34;
    signal("pad reading callbacks verified");
    return 0;
}

static int check(const char* resume_file) {
    ComPtr<IGameInput> input;
    if (FAILED(GameInputCreate(&input))) return 1;
    auto first = await_pad(input.Get(), nullptr);
    if (!first) return 2;
    ComPtr<IGameInputDevice> primary, secondary;
    first->GetDevice(&primary);
    signal("pad history first");
    auto baseline = await_pad(input.Get(), primary.Get());
    if (!baseline) return 3;
    baseline->GetDevice(&secondary);

    // Let the neutral readings age beyond the history window before the pause.
    Sleep(600);
    if (!at_end(input.Get(), baseline.Get(), nullptr)) return 4;
    if (!previous_fails(input.Get(), first.Get(), primary.Get(), GAMEINPUT_E_READING_NOT_FOUND) ||
        !previous_is(input.Get(), baseline.Get(), nullptr, first.Get())) return 40;
    signal("pad history paused");
    const auto deadline = GetTickCount64() + 8000;
    while (GetFileAttributesA(resume_file) == INVALID_FILE_ATTRIBUTES) {
        if (GetTickCount64() >= deadline) return 5;
        Sleep(5);
    }

    ComPtr<IGameInputReading> current;
    if (FAILED(input->GetCurrentReading(GameInputKindGamepad, primary.Get(), &current)) ||
        !matches(current.Get(), primary.Get(), false, false)) return 6;

    std::array<ComPtr<IGameInputReading>, 4> observed;
    auto cursor = baseline;
    for (size_t i = 0; i < observed.size(); ++i) {
        if (FAILED(input->GetNextReading(cursor.Get(), GameInputKindUnknown, nullptr, &observed[i]))) return 7;
        if (!matches(observed[i].Get(), i % 2 ? secondary.Get() : primary.Get(), i == 0, i == 1) ||
            observed[i]->GetTimestamp() < cursor->GetTimestamp()) return 8;
        cursor = observed[i];
    }
    if (!at_end(input.Get(), cursor.Get(), nullptr) || current != observed[2]) return 9;

    for (size_t pad = 0; pad < 2; ++pad) {
        auto device = pad ? secondary : primary;
        cursor = pad ? baseline : first;
        for (size_t i = pad; i < observed.size(); i += 2) {
            ComPtr<IGameInputReading> next;
            if (FAILED(input->GetNextReading(cursor.Get(), GameInputKindGamepad | GameInputKindMouse, device.Get(), &next)) ||
                next != observed[i]) return 10;
            cursor = next;
        }
        if (!at_end(input.Get(), cursor.Get(), device.Get())) return 11;
    }
    // Walk the same observations backward, checking COM identity at each step.
    for (size_t i = observed.size() - 1; i > 0; --i) {
        if (!previous_is(input.Get(), observed[i].Get(), nullptr, observed[i - 1].Get(), GameInputKindUnknown))
            return 41;
    }
    if (!previous_is(input.Get(), observed[3].Get(), secondary.Get(), observed[1].Get()) ||
        !previous_is(input.Get(), observed[2].Get(), primary.Get(), observed[0].Get(),
                     GameInputKindGamepad | GameInputKindMouse) ||
        !previous_is(input.Get(), observed[2].Get(), secondary.Get(), observed[1].Get())) return 42;
    if (!previous_fails(input.Get(), observed[0].Get(), nullptr, GAMEINPUT_E_READING_NOT_FOUND) ||
        !previous_fails(input.Get(), observed[3].Get(), nullptr, GAMEINPUT_E_READING_NOT_FOUND, GameInputKindKeyboard))
        return 43;
    ComPtr<IGameInputReading> result;
    if (input->GetNextReading(baseline.Get(), GameInputKindKeyboard, nullptr, &result) != GAMEINPUT_E_READING_NOT_FOUND || result)
        return 12;
    Sleep(650);
    if (input->GetNextReading(observed[0].Get(), GameInputKindGamepad, primary.Get(), &result) != GAMEINPUT_E_REFERENCE_READING_TOO_OLD ||
        result || !matches(observed[0].Get(), primary.Get(), true, false) ||
        !at_end(input.Get(), observed[2].Get(), primary.Get())) return 13;
    if (!previous_fails(input.Get(), observed[0].Get(), primary.Get(), GAMEINPUT_E_REFERENCE_READING_TOO_OLD) ||
        !previous_fails(input.Get(), observed[2].Get(), primary.Get(), GAMEINPUT_E_READING_NOT_FOUND) ||
        !previous_is(input.Get(), observed[3].Get(), nullptr, observed[2].Get())) return 44;
    signal("pad history replayed");
    int status = check_subscriptions(input.Get(), primary.Get(), secondary.Get());
    if (status) return status;
    // The secondary pad is idle across the final tap on the primary pad.
    // Its retained state is reachable by device, but is not the preceding
    // global event once the primary pad's press has expired.
    Sleep(650);
    ComPtr<IGameInputReading> primary_idle, secondary_idle;
    if (FAILED(input->GetCurrentReading(GameInputKindGamepad, primary.Get(), &primary_idle)) ||
        FAILED(input->GetCurrentReading(GameInputKindGamepad, secondary.Get(), &secondary_idle)) ||
        !previous_fails(input.Get(), primary_idle.Get(), nullptr, GAMEINPUT_E_READING_NOT_FOUND) ||
        !previous_is(input.Get(), primary_idle.Get(), secondary.Get(), secondary_idle.Get())) return 45;
    status = check_overrun(input.Get(), primary.Get());
    if (status) return status;
    const auto disconnect_deadline = GetTickCount64() + 8000;

    while (secondary->GetDeviceStatus() & GameInputDeviceConnected) {
        if (GetTickCount64() >= disconnect_deadline) return 14;
        Sleep(10);
    }
    if (input->GetNextReading(observed[3].Get(), GameInputKindGamepad, secondary.Get(), &result) != GAMEINPUT_E_DEVICE_DISCONNECTED ||
        result) return 15;
    if (!previous_fails(input.Get(), observed[3].Get(), secondary.Get(), GAMEINPUT_E_DEVICE_DISCONNECTED)) return 46;
    signal("pad history verified");
    return 0;
}

int main(int argc, char** argv) {
    int status = argc == 2 ? check(argv[1]) : 1;
    if (status) std::fprintf(stderr, "GameInput history probe failed at check %d\n", status);
    return status;
}
