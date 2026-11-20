// Linked by ordinal, independently of Coin Run's optional loader and fallback.
#include <windows.h>
#include <GameInput.h>
#include <wrl/client.h>
#include <cstdio>
using Microsoft::WRL::ComPtr;

static void signal(const char* message) {
    std::puts(message);
    std::fflush(stdout);
}

static bool wait_for_menu(IGameInput* input, IGameInputDevice* device, bool pressed) {
    const auto deadline = GetTickCount64() + 8000;
    do {
        ComPtr<IGameInputReading> reading;
        GameInputGamepadState state{};
        if (FAILED(input->GetCurrentReading(GameInputKindGamepad, device, &reading)) ||
            !reading->GetGamepadState(&state)) return false;
        if (bool(state.buttons & GameInputGamepadMenu) == pressed) return true;
        Sleep(10);
    } while (GetTickCount64() < deadline);
    return false;
}

static int probe() {
    ComPtr<IGameInput> input, second;
    if (FAILED(GameInputCreate(&input)) || !input || FAILED(GameInputCreate(&second)) ||
        second.Get() != input.Get()) return 1;
    ComPtr<IGameInputReading> baseline;
    ULONGLONG deadline = GetTickCount64() + 15000;
    while (FAILED(input->GetCurrentReading(GameInputKindGamepad, nullptr, &baseline))) {
        if (GetTickCount64() >= deadline) return 2;
        Sleep(16);
    }
    ComPtr<IGameInputDevice> device;
    baseline->GetDevice(&device);
    GameInputGamepadState initial{};
    if (!device || !(device->GetDeviceStatus() & GameInputDeviceConnected) ||
        !(device->GetDeviceInfo()->supportedInput & GameInputKindGamepad) ||
        !baseline->GetGamepadState(&initial) || initial.buttons != GameInputGamepadNone) return 3;
    if (device->GetDeviceInfo()->supportedRumbleMotors !=
            (GameInputRumbleLowFrequency | GameInputRumbleHighFrequency) ||
        !(device->GetDeviceStatus() & GameInputDeviceOutputEnabled)) return 12;
    signal("pad baseline");
    const auto mask = GameInputGamepadA | GameInputGamepadX | GameInputGamepadRightShoulder |
                      GameInputGamepadLeftThumbstick | GameInputGamepadView;
    ComPtr<IGameInputReading> held;
    deadline = GetTickCount64() + 8000;
    for (;;) {
        if (FAILED(input->GetCurrentReading(GameInputKindGamepad, device.Get(), &held))) return 4;
        GameInputGamepadState state{};
        if (!held->GetGamepadState(&state)) return 5;
        if (state.buttons == mask && state.leftTrigger > 0.95f && state.rightTrigger > 0.95f &&
            state.leftThumbstickX < -0.95f && state.leftThumbstickY > 0.95f &&
            state.rightThumbstickX > 0.95f && state.rightThumbstickY < -0.95f) break;
        if (GetTickCount64() >= deadline) {
            std::fprintf(stderr, "buttons %#x, triggers %.3f %.3f, sticks %.3f %.3f %.3f %.3f\n",
                unsigned(state.buttons), state.leftTrigger, state.rightTrigger, state.leftThumbstickX,
                state.leftThumbstickY, state.rightThumbstickX, state.rightThumbstickY);
            return 6;
        }
        Sleep(10);
    }
    GameInputGamepadState old{};
    if (!baseline->GetGamepadState(&old) || old.buttons != initial.buttons ||
        old.leftTrigger != initial.leftTrigger || old.rightThumbstickX != initial.rightThumbstickX ||
        held->GetTimestamp() <= baseline->GetTimestamp() || held->GetTimestamp() > input->GetCurrentTimestamp()) return 7;
    const GameInputRumbleParams trigger{-1.f, 0.f, 2.f, 0.f};
    device->SetRumbleState(&trigger);
    signal("pad trigger pulse");
    if (!wait_for_menu(input.Get(), device.Get(), true)) return 13;
    const GameInputRumbleParams strong{2.f, -1.f, 0.f, 0.f};
    device->SetRumbleState(&strong);
    signal("pad clamped pulse");
    if (!wait_for_menu(input.Get(), device.Get(), false)) return 14;
    device->SetRumbleState(nullptr);
    signal("pad sampled");
    deadline = GetTickCount64() + 8000;
    while (device->GetDeviceStatus() & GameInputDeviceConnected) {
        if (GetTickCount64() >= deadline) return 8;
        Sleep(16);
    }
    ComPtr<IGameInputReading> latest;
    if (input->GetCurrentReading(GameInputKindGamepad, device.Get(), &latest) != GAMEINPUT_E_DEVICE_DISCONNECTED ||
        latest || !held->GetGamepadState(&old) || old.buttons != mask) return 9;
    signal("pad detached");
    deadline = GetTickCount64() + 8000;
    while (FAILED(input->GetCurrentReading(GameInputKindGamepad, nullptr, &latest))) {
        if (GetTickCount64() >= deadline) return 10;
        Sleep(16);
    }
    ComPtr<IGameInputDevice> replacement;
    latest->GetDevice(&replacement);
    if (!replacement || replacement.Get() == device.Get() ||
        device->GetDeviceStatus() != GameInputDeviceNoStatus ||
        input->GetCurrentReading(GameInputKindGamepad, device.Get(), &latest) != GAMEINPUT_E_DEVICE_DISCONNECTED ||
        latest) return 11;
    // Let the fixture observe a live effect throughout commands on the old handle.
    const GameInputRumbleParams weak{0.f, 0.5f, 0.f, 0.f};
    replacement->SetRumbleState(&weak);
    signal("pad replacement pulse");
    if (!wait_for_menu(input.Get(), replacement.Get(), true)) return 15;
    device->SetRumbleState(&strong);
    device->SetRumbleState(nullptr);
    signal("pad stale handle checked");
    if (!wait_for_menu(input.Get(), replacement.Get(), false)) return 16;
    replacement->SetRumbleState(nullptr);
    signal("pad replaced");
    return 0;
}

int main() {
    int status = probe();
    if (status) std::fprintf(stderr, "GameInput ordinal probe failed at check %d\n", status);
    return status;
}
