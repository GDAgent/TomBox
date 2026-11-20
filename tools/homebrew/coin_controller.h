#pragma once
#include <windows.h>
#include <GameInput.h>
#include "../../crates/compat/gameinput/xinput_state.h"
#include <wrl/client.h>
#include <algorithm>
#include <cmath>
#include <utility>

// One action per press or stick deflection. A new connection or focus change
// must first deliver a neutral sample, so a held control cannot replay a move.
struct CoinControls {
    bool armed = false;
    char previous = 0;

    char update(const GameInputGamepadState& state) {
        const auto buttons = state.buttons;
        char action = 0;
        if (buttons & GameInputGamepadMenu) action = 'r';
        else if (buttons & (GameInputGamepadDPadUp | GameInputGamepadDPadDown |
                            GameInputGamepadDPadLeft | GameInputGamepadDPadRight)) {
            switch (buttons & (GameInputGamepadDPadUp | GameInputGamepadDPadDown |
                               GameInputGamepadDPadLeft | GameInputGamepadDPadRight)) {
                case GameInputGamepadDPadUp: action = 'w'; break;
                case GameInputGamepadDPadDown: action = 's'; break;
                case GameInputGamepadDPadLeft: action = 'a'; break;
                case GameInputGamepadDPadRight: action = 'd'; break;
                default: return 0; // Wait for a single D-pad direction.
            }
        } else {
            float x = state.leftThumbstickX, y = state.leftThumbstickY;
            float magnitude = (std::max)(std::fabs(x), std::fabs(y));
            if (magnitude >= 0.55f)
                action = std::fabs(x) > std::fabs(y) ? (x < 0 ? 'a' : 'd') : (y < 0 ? 's' : 'w');
            else if (magnitude > 0.35f) return 0; // Hysteresis near the activation threshold.
        }
        if (!armed) {
            armed = !action;
            return 0;
        }
        if (action == previous) return 0;
        previous = action;
        return action;
    }
};

class CoinController {
    const bool allow_xinput;
    HMODULE library = nullptr;
    Microsoft::WRL::ComPtr<IGameInput> input;
    Microsoft::WRL::ComPtr<IGameInputDevice> device;
    Microsoft::WRL::ComPtr<IGameInputReading> cursor;
    bool can_walk = true;
    CoinControls controls;
    DWORD slot = XUSER_MAX_COUNT;
    ULONGLONG next_scan = 0;
    ULONGLONG rumble_until = 0;

    void rumble(const GameInputRumbleParams* params) {
        if (device) device->SetRumbleState(params);
        else if (slot < XUSER_MAX_COUNT) {
            auto vibration = xinput_vibration(params);
            XInputSetState(slot, &vibration);
        }
    }
    void stop_rumble() {
        if (rumble_until) rumble(nullptr);
        rumble_until = 0;
    }

    bool read_xinput(GameInputGamepadState& sample) {
        XINPUT_STATE state{};
        if (slot < XUSER_MAX_COUNT) {
            if (XInputGetState(slot, &state) != ERROR_SUCCESS) {
                rumble_until = 0;
                slot = XUSER_MAX_COUNT;
                return false;
            }
        } else {
            ULONGLONG now = GetTickCount64();
            if (now < next_scan) return false;
            next_scan = now + 500;
            for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i) {
                if (XInputGetState(i, &state) == ERROR_SUCCESS) { slot = i; break; }
            }
            if (slot == XUSER_MAX_COUNT) return false;
        }
        sample = gamepad_state(state.Gamepad);
        return true;
    }

public:
    explicit CoinController(bool allow_xinput) : allow_xinput(allow_xinput) {
        // Keep keyboard play available on systems without the optional GameInput runtime.
        library = LoadLibraryExW(L"gameinput.dll", nullptr, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (library) {
            auto create = reinterpret_cast<decltype(&GameInputCreate)>(GetProcAddress(library, "GameInputCreate"));
            if (create && FAILED(create(input.GetAddressOf()))) input.Reset();
        }
    }
    ~CoinController() {
        stop_rumble();
        cursor.Reset();
        device.Reset();
        input.Reset();
        if (library) FreeLibrary(library);
    }
    CoinController(const CoinController&) = delete;
    CoinController& operator=(const CoinController&) = delete;

    bool available() const { return input || allow_xinput; }
    const char* backend() const { return input ? "GameInput" : (allow_xinput ? "XInput" : "unavailable"); }

    bool ready() const { return controls.armed; }
    bool pressed() const { return controls.previous != 0; }
    void reset() { stop_rumble(); controls = {}; cursor.Reset(); }
    void pickup() {
        const GameInputRumbleParams pulse{0.625f, 0.25f, 0.f, 0.f};
        rumble(&pulse);
        rumble_until = GetTickCount64() + 180;
    }

    char poll() {
        if (rumble_until && GetTickCount64() >= rumble_until) stop_rumble();
        GameInputGamepadState state{};
        if (input) {
            Microsoft::WRL::ComPtr<IGameInputReading> reading;
            HRESULT hr;
            for (;;) {
                if (cursor) {
                    hr = input->GetNextReading(cursor.Get(), GameInputKindGamepad, device.Get(), &reading);
                    if (hr == GAMEINPUT_E_READING_NOT_FOUND && (device->GetDeviceStatus() & GameInputDeviceConnected))
                        return 0;
                    if (hr == E_NOTIMPL) can_walk = false;
                    if (hr == GAMEINPUT_E_REFERENCE_READING_TOO_OLD || !can_walk) reset();
                }
                if (!cursor) hr = input->GetCurrentReading(GameInputKindGamepad, device.Get(), &reading);
                if (FAILED(hr) || !reading || !reading->GetGamepadState(&state)) break;
                if (!device) reading->GetDevice(&device);
                if (!device || !(device->GetDeviceStatus() & GameInputDeviceConnected)) break;
                const bool was_pressed = pressed();
                const char action = controls.update(state);
                if (!can_walk) return action;
                cursor = std::move(reading);
                // Consume analog noise now. Deliver distinct actions and releases
                // to the board in order, one edge per window update.
                if (action || (was_pressed && !pressed())) return action;
            }
            reset();
            device.Reset();
            if (hr != E_NOTIMPL) return 0;
            input.Reset();
        }
        if (allow_xinput && read_xinput(state)) return controls.update(state);
        controls = {};
        return 0;
    }
};
