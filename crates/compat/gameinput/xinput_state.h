#pragma once
#include <windows.h>
#include <GameInput.h>
#include <Xinput.h>

inline GameInputGamepadState gamepad_state(const XINPUT_GAMEPAD& pad) {
    static constexpr struct { WORD source; GameInputGamepadButtons target; } buttons[] = {
        {XINPUT_GAMEPAD_START, GameInputGamepadMenu}, {XINPUT_GAMEPAD_BACK, GameInputGamepadView},
        {XINPUT_GAMEPAD_A, GameInputGamepadA}, {XINPUT_GAMEPAD_B, GameInputGamepadB},
        {XINPUT_GAMEPAD_X, GameInputGamepadX}, {XINPUT_GAMEPAD_Y, GameInputGamepadY},
        {XINPUT_GAMEPAD_DPAD_UP, GameInputGamepadDPadUp}, {XINPUT_GAMEPAD_DPAD_DOWN, GameInputGamepadDPadDown},
        {XINPUT_GAMEPAD_DPAD_LEFT, GameInputGamepadDPadLeft}, {XINPUT_GAMEPAD_DPAD_RIGHT, GameInputGamepadDPadRight},
        {XINPUT_GAMEPAD_LEFT_SHOULDER, GameInputGamepadLeftShoulder}, {XINPUT_GAMEPAD_RIGHT_SHOULDER, GameInputGamepadRightShoulder},
        {XINPUT_GAMEPAD_LEFT_THUMB, GameInputGamepadLeftThumbstick}, {XINPUT_GAMEPAD_RIGHT_THUMB, GameInputGamepadRightThumbstick},
    };
    GameInputGamepadState result{};
    for (auto button : buttons)
        if (pad.wButtons & button.source) result.buttons |= button.target;
    auto axis = [](SHORT value) { return value / (value < 0 ? 32768.f : 32767.f); };
    result.leftTrigger = pad.bLeftTrigger / 255.f;
    result.rightTrigger = pad.bRightTrigger / 255.f;
    result.leftThumbstickX = axis(pad.sThumbLX);
    result.leftThumbstickY = axis(pad.sThumbLY);
    result.rightThumbstickX = axis(pad.sThumbRX);
    result.rightThumbstickY = axis(pad.sThumbRY);
    return result;
}

// XInput exposes two body motors. Mix trigger feedback into the high-frequency
// channel at one quarter strength per trigger when dedicated motors are absent.
inline XINPUT_VIBRATION xinput_vibration(const GameInputRumbleParams* params) {
    if (!params) return {};
    auto unit = [](float v) { return v > 0.f ? (v < 1.f ? v : 1.f) : 0.f; };
    auto speed = [&](float v) { return static_cast<WORD>(unit(v) * 65535.f + 0.5f); };
    const float triggers = unit(params->leftTrigger) + unit(params->rightTrigger);
    return {speed(params->lowFrequency), speed(unit(params->highFrequency) + 0.25f * triggers)};
}
