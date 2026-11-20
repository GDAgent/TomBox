// Gamepad subset of the public Windows SDK GameInput v0 ABI.
#include "xinput_state.h"
#include <algorithm>
#include <mutex>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <iterator>
#include <vector>
#include <wrl/implements.h>
#include <wrl/client.h>

using namespace Microsoft::WRL;
template<class Interface>
using ComObject = RuntimeClass<RuntimeClassFlags<ClassicCom>, Interface>;

extern "C" IMAGE_DOS_HEADER __ImageBase;

namespace {
uint64_t clock_us() {
    static const auto frequency = [] {
        LARGE_INTEGER value;
        QueryPerformanceFrequency(&value);
        return value.QuadPart;
    }();
    LARGE_INTEGER counter;
    QueryPerformanceCounter(&counter);
    return counter.QuadPart / frequency * 1000000 +
           counter.QuadPart % frequency * 1000000 / frequency;
}

template<class T>
HRESULT unsupported(T** out) {
    if (!out) return E_POINTER;
    *out = nullptr;
    return E_NOTIMPL;
}

class Device final : public ComObject<IGameInputDevice> {
    const DWORD slot;
    // Serialize output with disconnect observation before this slot can be reused.
    std::mutex port_mutex;
    bool connected = true;
    GameInputDeviceInfo info{};
public:
    // Highest discarded reading in this connection's stream (under Input::mutex).
    uint64_t last_expired_order = 0;

    explicit Device(DWORD index) : slot(index) {
        static constexpr GameInputGamepadInfo labels = {
            GameInputLabelXboxMenu, GameInputLabelXboxView, GameInputLabelXboxA, GameInputLabelXboxB,
            GameInputLabelXboxX, GameInputLabelXboxY, GameInputLabelXboxDPadUp, GameInputLabelXboxDPadDown,
            GameInputLabelXboxDPadLeft, GameInputLabelXboxDPadRight, GameInputLabelXboxLeftShoulder,
            GameInputLabelXboxRightShoulder, GameInputLabelXboxLeftStickButton, GameInputLabelXboxRightStickButton
        };
        info.infoSize = sizeof(info);
        // XInput exposes virtual player slots. These IDs identify ports, not hardware.
        info.deviceId.value[0] = static_cast<BYTE>(index + 1);
        info.deviceRootId = info.deviceId;
        info.deviceFamily = GameInputFamilyVirtual;
        info.supportedInput = GameInputKindGamepad;
        info.gamepadInfo = &labels;
        XINPUT_CAPABILITIES caps{};
        if (XInputGetCapabilities(slot, 0, &caps) == ERROR_SUCCESS) {
            if (caps.Vibration.wLeftMotorSpeed) info.supportedRumbleMotors |= GameInputRumbleLowFrequency;
            if (caps.Vibration.wRightMotorSpeed) info.supportedRumbleMotors |= GameInputRumbleHighFrequency;
        }
    }
    bool sample(XINPUT_STATE& state) {
        std::lock_guard<std::mutex> lock(port_mutex);
        if (!connected) return false;
        if (XInputGetState(slot, &state) == ERROR_SUCCESS) return true;
        connected = false;
        return false;
    }
    GameInputDeviceStatus online_status() const {
        auto status = GameInputDeviceConnected | GameInputDeviceInputEnabled;
        if (info.supportedRumbleMotors) status |= GameInputDeviceOutputEnabled;
        return status;
    }
    const GameInputDeviceInfo* STDMETHODCALLTYPE GetDeviceInfo() noexcept override { return &info; }
    GameInputDeviceStatus STDMETHODCALLTYPE GetDeviceStatus() noexcept override {
        XINPUT_STATE state;
        return sample(state) ? online_status() : GameInputDeviceNoStatus;
    }
    void STDMETHODCALLTYPE GetBatteryState(GameInputBatteryState* out) noexcept override {
        if (out) { *out = {}; out->status = GameInputBatteryUnknown; }
    }
    HRESULT STDMETHODCALLTYPE CreateForceFeedbackEffect(uint32_t, const GameInputForceFeedbackParams*, IGameInputForceFeedbackEffect** out) noexcept override { return unsupported(out); }
    bool STDMETHODCALLTYPE IsForceFeedbackMotorPoweredOn(uint32_t) noexcept override { return false; }
    void STDMETHODCALLTYPE SetForceFeedbackMotorGain(uint32_t, float) noexcept override {}
    HRESULT STDMETHODCALLTYPE SetHapticMotorState(uint32_t, const GameInputHapticFeedbackParams*) noexcept override { return E_NOTIMPL; }
    void STDMETHODCALLTYPE SetRumbleState(const GameInputRumbleParams* params) noexcept override {
        std::lock_guard<std::mutex> lock(port_mutex);
        if (!connected || !info.supportedRumbleMotors) return;
        auto vibration = xinput_vibration(params);
        if (XInputSetState(slot, &vibration) == ERROR_DEVICE_NOT_CONNECTED) connected = false;
    }
    void STDMETHODCALLTYPE SetInputSynchronizationState(bool) noexcept override {}
    void STDMETHODCALLTYPE SendInputSynchronizationHint() noexcept override {}
    void STDMETHODCALLTYPE PowerOff() noexcept override {}
    HRESULT STDMETHODCALLTYPE CreateRawDeviceReport(uint32_t, GameInputRawDeviceReportKind, IGameInputRawDeviceReport** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE GetRawDeviceFeature(uint32_t, IGameInputRawDeviceReport** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE SetRawDeviceFeature(IGameInputRawDeviceReport*) noexcept override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE SendRawDeviceOutput(IGameInputRawDeviceReport*) noexcept override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE SendRawDeviceOutputWithResponse(IGameInputRawDeviceReport*, IGameInputRawDeviceReport** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE ExecuteRawDeviceIoControl(uint32_t, size_t, const void*, size_t, void*, size_t* count) noexcept override {
        if (count) *count = 0;
        return E_NOTIMPL;
    }
    bool STDMETHODCALLTYPE AcquireExclusiveRawDeviceAccess(uint64_t) noexcept override { return false; }
    void STDMETHODCALLTYPE ReleaseExclusiveRawDeviceAccess() noexcept override {}
};

class Reading final : public ComObject<IGameInputReading> {
    const ComPtr<Device> device;
    const GameInputGamepadState state;
    const uint64_t timestamp;
public:
    const uint64_t order;
    uint64_t retired_at = 0; // Bookkeeping under Input::mutex. The sample stays immutable.
    Reading(Device* source, const GameInputGamepadState& sample, uint64_t order)
        : device(source), state(sample), timestamp(clock_us()), order(order) {}
    bool matches(const GameInputGamepadState& sample) const {
        return state.buttons == sample.buttons && state.leftTrigger == sample.leftTrigger &&
               state.rightTrigger == sample.rightTrigger && state.leftThumbstickX == sample.leftThumbstickX &&
               state.leftThumbstickY == sample.leftThumbstickY && state.rightThumbstickX == sample.rightThumbstickX &&
               state.rightThumbstickY == sample.rightThumbstickY;
    }
    bool meets_threshold(const Reading& baseline, float threshold) const {
        const auto& old = baseline.state;
        return state.buttons != old.buttons ||
            std::abs(state.leftTrigger - old.leftTrigger) >= threshold ||
            std::abs(state.rightTrigger - old.rightTrigger) >= threshold ||
            std::abs(state.leftThumbstickX - old.leftThumbstickX) >= threshold ||
            std::abs(state.leftThumbstickY - old.leftThumbstickY) >= threshold ||
            std::abs(state.rightThumbstickX - old.rightThumbstickX) >= threshold ||
            std::abs(state.rightThumbstickY - old.rightThumbstickY) >= threshold;
    }
    Device* source() const { return device.Get(); }
    uint64_t STDMETHODCALLTYPE GetTimestamp() noexcept override { return timestamp; }
    // Microsoft's v0 SDK reports zero for all sequence numbers.
    uint64_t STDMETHODCALLTYPE GetSequenceNumber(GameInputKind) noexcept override { return 0; }
    GameInputKind STDMETHODCALLTYPE GetInputKind() noexcept override { return GameInputKindGamepad; }
    void STDMETHODCALLTYPE GetDevice(IGameInputDevice** out) noexcept override { if (out) { device->AddRef(); *out = device.Get(); } }
    bool STDMETHODCALLTYPE GetGamepadState(GameInputGamepadState* out) noexcept override {
        if (!out) return false;
        *out = state;
        return true;
    }
    bool STDMETHODCALLTYPE GetRawReport(IGameInputRawDeviceReport** out) noexcept override { if (out) *out = nullptr; return false; }
    uint32_t STDMETHODCALLTYPE GetControllerAxisCount() noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetControllerAxisState(uint32_t, float*) noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetControllerButtonCount() noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetControllerButtonState(uint32_t, bool*) noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetControllerSwitchCount() noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetControllerSwitchState(uint32_t, GameInputSwitchPosition*) noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetKeyCount() noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetKeyState(uint32_t, GameInputKeyState*) noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetTouchCount() noexcept override { return 0; }
    uint32_t STDMETHODCALLTYPE GetTouchState(uint32_t, GameInputTouchState*) noexcept override { return 0; }
    bool STDMETHODCALLTYPE GetMouseState(GameInputMouseState*) noexcept override { return false; }
    bool STDMETHODCALLTYPE GetMotionState(GameInputMotionState*) noexcept override { return false; }
    bool STDMETHODCALLTYPE GetArcadeStickState(GameInputArcadeStickState*) noexcept override { return false; }
    bool STDMETHODCALLTYPE GetFlightStickState(GameInputFlightStickState*) noexcept override { return false; }
    bool STDMETHODCALLTYPE GetRacingWheelState(GameInputRacingWheelState*) noexcept override { return false; }
    bool STDMETHODCALLTYPE GetUiNavigationState(GameInputUiNavigationState*) noexcept override { return false; }
};

class Input final : public ComObject<IGameInput> {
    struct Slot {
        ComPtr<Device> device;
        ComPtr<Reading> reading;
        DWORD packet = 0;
    } slots[XUSER_MAX_COUNT];
    std::mutex mutex;
    std::vector<ComPtr<Reading>> history;
    uint64_t next_order = 1, last_expired_order = 0;
    bool sampling = false;

    static constexpr uint64_t history_us = 500000;

    void expire_history(uint64_t now) {
        auto end = std::remove_if(history.begin(), history.end(), [&](const ComPtr<Reading>& r) {
            // The most recent state may have been held indefinitely. Keep its cursor
            // for half a second after it is superseded so the first change is reachable.
            if (!r->retired_at || now - r->retired_at < history_us) return false;
            last_expired_order = (std::max)(last_expired_order, r->order);
            auto& per_device = r->source()->last_expired_order;
            per_device = (std::max)(per_device, r->order);
            return true;
        });
        history.erase(end, history.end());
    }

    struct Registration {
        GameInputCallbackToken token = 0;
        ComPtr<IGameInputDevice> filter;
        GameInputKind kinds{};
        GameInputDeviceStatus statuses{};
        GameInputDeviceCallback device_callback = nullptr;
        void* context = nullptr;
        unsigned initial_pending = 0;
        bool stopped = false;

        GameInputReadingCallback reading_callback = nullptr;
        float threshold = 0;
        ComPtr<Reading> baseline[XUSER_MAX_COUNT];
        unsigned pending_readings = 0;
        bool missed_reading = false;

        bool matches(Device* device) const {
            return token && !stopped && (!filter || filter.Get() == device) &&
                (kinds == GameInputKindUnknown || (kinds & GameInputKindGamepad));
        }
        bool matches_status(Device* device) const {
            return device_callback && matches(device) && (statuses & device->online_status());
        }
    } registrations[64];

    struct Event {
        GameInputCallbackToken token;
        ComPtr<Device> device;
        uint64_t timestamp;
        bool connected, initial;
        ComPtr<Reading> reading;
        bool overrun = false;
    };
    std::vector<Event> pending;
    std::condition_variable changed;
    GameInputCallbackToken next_token = 1, executing = 0;
    DWORD dispatch_thread = 0;
    bool worker_started = false;

    Registration* find(GameInputCallbackToken token) {
        for (auto& r : registrations)
            if (r.token == token) return &r;
        return nullptr;
    }

    // Queue before publishing a slot transition so allocation failure can be retried.
    HRESULT transition(Device* device, bool connected) {
        const auto start = pending.size();
        const auto time = clock_us();
        try {
            for (const auto& r : registrations)
                if (r.matches_status(device)) pending.push_back({r.token, device, time, connected, false});
        } catch (const std::bad_alloc&) {
            pending.resize(start);
            return E_OUTOFMEMORY;
        }
        if (pending.size() != start) changed.notify_all();
        return S_OK;
    }

    // Keep the newest state when a consumer blocks the dispatcher. Move a
    // replacement to the end so it stays ordered with other callbacks.
    void queue_reading(DWORD index, const ComPtr<Reading>& reading) {
        constexpr unsigned max_pending_readings = 64;
        for (auto& r : registrations) {
            if (!r.reading_callback || !r.matches(reading->source())) continue;
            const auto& baseline = r.baseline[index];
            if (!r.missed_reading && r.threshold && baseline && baseline->source() == reading->source() &&
                !reading->meets_threshold(*baseline.Get(), r.threshold)) continue;

            if (r.pending_readings == max_pending_readings) {
                auto last = std::find_if(pending.rbegin(), pending.rend(),
                                         [&](const Event& e) { return e.token == r.token; });
                std::rotate(std::prev(last.base()), last.base(), pending.end());
                pending.back().reading = reading;
                pending.back().overrun = true;
            } else {
                try {
                    pending.push_back({r.token, nullptr, 0, false, false, reading, r.missed_reading});
                } catch (const std::bad_alloc&) {
                    // History remains intact; report the lost notification on the next delivery.
                    r.missed_reading = true;
                    continue;
                }
                ++r.pending_readings;
            }
            if (r.threshold) r.baseline[index] = reading;
            r.missed_reading = false;
        }
        if (!pending.empty()) changed.notify_all();
    }

    HRESULT refresh_slot(DWORD i) {
        auto& slot = slots[i];
        XINPUT_STATE state;
        if (slot.device) {
            if (!slot.device->sample(state)) {
                HRESULT hr = transition(slot.device.Get(), false);
                if (SUCCEEDED(hr)) {
                    if (slot.reading) slot.reading->retired_at = clock_us();
                    slot = {};
                }
                return hr;
            }
        } else {
            if (XInputGetState(i, &state) != ERROR_SUCCESS) return S_OK;
            auto device = Make<Device>(i);
            if (!device) return E_OUTOFMEMORY;
            HRESULT hr = transition(device.Get(), true);
            if (FAILED(hr)) return hr;
            slot.device = std::move(device);
        }
        if (slot.reading && slot.packet == state.dwPacketNumber) return S_OK;
        const auto sample = gamepad_state(state.Gamepad);
        if (!slot.reading || !slot.reading->matches(sample)) {
            auto reading = Make<Reading>(slot.device.Get(), sample, next_order);
            if (!reading) return E_OUTOFMEMORY;
            try {
                history.push_back(reading);
            } catch (const std::bad_alloc&) {
                return E_OUTOFMEMORY;
            }
            ++next_order;
            if (slot.reading) slot.reading->retired_at = reading->GetTimestamp();
            slot.reading = std::move(reading);
            queue_reading(i, slot.reading);
        }
        slot.packet = state.dwPacketNumber;
        return S_OK;
    }

    bool callbacks_active() const {
        return std::any_of(std::begin(registrations), std::end(registrations),
                           [](const Registration& r) { return r.token && !r.stopped; });
    }

    static void CALLBACK sample_worker(PTP_CALLBACK_INSTANCE instance, void* context) {
        FreeLibraryWhenCallbackReturns(instance, reinterpret_cast<HMODULE>(&__ImageBase));
        auto& input = *static_cast<Input*>(context);
        std::unique_lock<std::mutex> lock(input.mutex);
        // The factory holds one permanent reference; the others belong to consumers.
        uint64_t next_discovery = 0;
        while (input.GetRefCount() > 1 || input.callbacks_active()) {
            const auto now = clock_us();
            const bool discover = now >= next_discovery;
            if (discover) next_discovery = now + 32000;
            for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i)
                if (discover || input.slots[i].device) input.refresh_slot(i);
            input.expire_history(clock_us());
            lock.unlock();
            Sleep(8);
            lock.lock();
        }
        input.sampling = false;
    }

    void dispatch() {
        std::vector<Event> batch;
        std::unique_lock<std::mutex> lock(mutex);
        dispatch_thread = GetCurrentThreadId();
        while (callbacks_active()) {
            changed.wait(lock, [&] { return !pending.empty() || !callbacks_active(); });
            batch.swap(pending);
            for (auto& r : registrations) r.pending_readings = 0;
            for (const auto& event : batch) {
                auto* r = find(event.token);
                if (!r || r->stopped) continue;
                executing = r->token;
                auto device_callback = r->device_callback;
                auto reading_callback = r->reading_callback;
                auto context = r->context;
                lock.unlock();
                if (event.reading) {
                    reading_callback(event.token, context, event.reading.Get(), event.overrun);
                } else {
                    const auto online = event.device->online_status();
                    device_callback(event.token, context, event.device.Get(), event.timestamp,
                                    event.connected ? online : GameInputDeviceNoStatus,
                                    event.connected ? GameInputDeviceNoStatus : online);
                }
                lock.lock();
                // Unregister cannot recycle this registration while it is executing.
                if (event.initial) --r->initial_pending;
                executing = 0;
                changed.notify_all();
            }
            batch.clear();
        }
        pending.clear();
        dispatch_thread = 0;
        worker_started = false;
    }

    static void CALLBACK worker(PTP_CALLBACK_INSTANCE instance, void* context) {
        // Let the pool release the module after this callback returns through DLL code.
        FreeLibraryWhenCallbackReturns(instance, reinterpret_cast<HMODULE>(&__ImageBase));
        static_cast<Input*>(context)->dispatch();
    }

    HRESULT start_worker(PTP_SIMPLE_CALLBACK entry, bool& started) {
        if (started) return S_OK;
        HMODULE module;
        if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                                reinterpret_cast<LPCWSTR>(&__ImageBase), &module))
            return HRESULT_FROM_WIN32(GetLastError());
        if (!TrySubmitThreadpoolCallback(entry, this, nullptr)) {
            HRESULT hr = HRESULT_FROM_WIN32(GetLastError());
            FreeLibrary(module);
            return hr;
        }
        started = true;
        return S_OK;
    }

    HRESULT prepare_registration(Registration*& r) {
        r = find(0);
        if (!r) return HRESULT_FROM_WIN32(ERROR_TOO_MANY_CMDS);
        // Observe pre-existing state before installing any subscription.
        for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i) {
            HRESULT hr = refresh_slot(i);
            if (FAILED(hr)) return hr;
        }
        return start_worker(worker, worker_started);
    }

    void stop(Registration& r) {
        r.stopped = true;
        pending.erase(std::remove_if(pending.begin(), pending.end(),
                                    [&](const Event& e) { return e.token == r.token; }), pending.end());
        r.pending_readings = 0;
        changed.notify_all();
    }

    enum class Direction { previous, next };

    HRESULT adjacent_reading(Direction direction, IGameInputReading* reference, GameInputKind kind,
                             IGameInputDevice* filter, IGameInputReading** out) {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (!reference) return E_INVALIDARG;
        if (kind != GameInputKindUnknown && !(kind & GameInputKindGamepad)) return GAMEINPUT_E_READING_NOT_FOUND;
        std::lock_guard<std::mutex> lock(mutex);
        Device* connected = nullptr;
        for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i) {
            if (filter && filter != slots[i].device.Get()) continue;
            HRESULT hr = refresh_slot(i);
            if (FAILED(hr)) return hr;
            if (filter) connected = slots[i].device.Get();
        }
        if (filter && !connected) return GAMEINPUT_E_DEVICE_DISCONNECTED;
        const auto now = clock_us();
        expire_history(now);
        const auto cursor = std::find_if(history.begin(), history.end(),
            [&](const ComPtr<Reading>& r) { return r.Get() == reference; });
        if (cursor == history.end()) return GAMEINPUT_E_REFERENCE_READING_TOO_OLD;

        const auto matches_device = [&](const ComPtr<Reading>& r) { return !filter || r->source() == filter; };
        Reading* candidate;
        if (direction == Direction::next) {
            const auto watermark = connected ? connected->last_expired_order : last_expired_order;
            if ((*cursor)->order < watermark) return GAMEINPUT_E_REFERENCE_READING_TOO_OLD;
            const auto next = std::find_if(std::next(cursor), history.end(), matches_device);
            if (next == history.end()) return GAMEINPUT_E_READING_NOT_FOUND;
            candidate = next->Get();
        } else {
            const auto previous = std::find_if(std::make_reverse_iterator(cursor), history.rend(), matches_device);
            if (previous == history.rend()) return GAMEINPUT_E_READING_NOT_FOUND;
            candidate = previous->Get();
            // An idle pad's current reading may outlive intervening readings on
            // another pad. Never cross that gap when walking the combined stream.
            // Within one connection readings always expire oldest first.
            if (!filter && candidate->order != (*cursor)->order - 1) return GAMEINPUT_E_READING_NOT_FOUND;
        }

        if (candidate->retired_at && now - candidate->GetTimestamp() > history_us)
            return direction == Direction::next ? GAMEINPUT_E_REFERENCE_READING_TOO_OLD : GAMEINPUT_E_READING_NOT_FOUND;
        candidate->AddRef();
        *out = candidate;
        return S_OK;
    }

    static HRESULT no_callback(GameInputCallbackToken* token) {
        if (token) *token = 0;
        return E_NOTIMPL;
    }
public:
    HRESULT open(IGameInput** out) {
        std::lock_guard<std::mutex> lock(mutex);
        HRESULT hr = start_worker(sample_worker, sampling);
        if (FAILED(hr)) return hr;
        AddRef();
        *out = this;
        return S_OK;
    }
    uint64_t STDMETHODCALLTYPE GetCurrentTimestamp() noexcept override { return clock_us(); }
    HRESULT STDMETHODCALLTYPE GetCurrentReading(GameInputKind kind, IGameInputDevice* filter, IGameInputReading** out) noexcept override {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (!(kind & GameInputKindGamepad) && kind != GameInputKindUnknown) return GAMEINPUT_E_READING_NOT_FOUND;
        std::lock_guard<std::mutex> guard(mutex);
        Reading* newest = nullptr;
        for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i) {
            auto& slot = slots[i];
            if (filter && filter != slot.device.Get()) continue;
            HRESULT hr = refresh_slot(i);
            if (FAILED(hr)) return hr;
            if (slot.reading && (!newest || slot.reading->order > newest->order))
                newest = slot.reading.Get();
        }
        if (!newest) return filter ? GAMEINPUT_E_DEVICE_DISCONNECTED : GAMEINPUT_E_READING_NOT_FOUND;
        newest->AddRef();
        *out = newest;
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE GetNextReading(IGameInputReading* reference, GameInputKind kind,
            IGameInputDevice* filter, IGameInputReading** out) noexcept override {
        return adjacent_reading(Direction::next, reference, kind, filter, out);
    }
    HRESULT STDMETHODCALLTYPE GetPreviousReading(IGameInputReading* reference, GameInputKind kind,
            IGameInputDevice* filter, IGameInputReading** out) noexcept override {
        return adjacent_reading(Direction::previous, reference, kind, filter, out);
    }
    HRESULT STDMETHODCALLTYPE GetTemporalReading(uint64_t, IGameInputDevice*, IGameInputReading** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE RegisterReadingCallback(IGameInputDevice* filter, GameInputKind kinds, float threshold,
            void* context, GameInputReadingCallback callback, GameInputCallbackToken* out) noexcept override {
        if (out) *out = 0;
        if (!callback || !std::isfinite(threshold) || threshold < 0) return E_INVALIDARG;
        std::lock_guard<std::mutex> lock(mutex);
        Registration* r;
        HRESULT hr = prepare_registration(r);
        if (FAILED(hr)) return hr;
        r->token = next_token++;
        r->filter = filter;
        r->kinds = kinds;
        r->context = context;
        r->reading_callback = callback;
        r->threshold = threshold;
        if (threshold) {
            for (DWORD i = 0; i < XUSER_MAX_COUNT; ++i)
                if (r->matches(slots[i].device.Get())) r->baseline[i] = slots[i].reading;
        }
        if (out) *out = r->token;
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE RegisterDeviceCallback(IGameInputDevice* filter, GameInputKind kinds,
            GameInputDeviceStatus statuses, GameInputEnumerationKind enumeration, void* context,
            GameInputDeviceCallback callback, GameInputCallbackToken* out) noexcept override {
        if (out) *out = 0;
        if (!callback || (enumeration != GameInputNoEnumeration && enumeration != GameInputAsyncEnumeration &&
                          enumeration != GameInputBlockingEnumeration)) return E_INVALIDARG;
        std::unique_lock<std::mutex> lock(mutex);
        // A callback cannot synchronously wait for its own serial dispatcher.
        if (dispatch_thread == GetCurrentThreadId() && enumeration == GameInputBlockingEnumeration)
            return HRESULT_FROM_WIN32(ERROR_POSSIBLE_DEADLOCK);
        Registration* r;
        HRESULT hr = prepare_registration(r);
        if (FAILED(hr)) return hr;
        const auto token = next_token++;
        *r = {token, filter, kinds, statuses, callback, context};
        const auto start = pending.size();
        if (enumeration != GameInputNoEnumeration) {
            try {
                for (const auto& slot : slots) {
                    if (slot.device && r->matches_status(slot.device.Get())) {
                        pending.push_back({token, slot.device, clock_us(), true, true});
                        ++r->initial_pending;
                    }
                }
            } catch (const std::bad_alloc&) {
                pending.resize(start);
                *r = {};
                return E_OUTOFMEMORY;
            }
        }
        if (out) *out = token;
        changed.notify_all();
        if (enumeration == GameInputBlockingEnumeration)
            changed.wait(lock, [&] { return r->token != token || !r->initial_pending ||
                                            (r->stopped && executing != token); });
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE RegisterSystemButtonCallback(IGameInputDevice*, GameInputSystemButtons, void*, GameInputSystemButtonCallback, GameInputCallbackToken* token) noexcept override { return no_callback(token); }
    HRESULT STDMETHODCALLTYPE RegisterKeyboardLayoutCallback(IGameInputDevice*, void*, GameInputKeyboardLayoutCallback, GameInputCallbackToken* token) noexcept override { return no_callback(token); }
    void STDMETHODCALLTYPE StopCallback(GameInputCallbackToken token) noexcept override {
        std::lock_guard<std::mutex> guard(mutex);
        if (token == GAMEINPUT_CURRENT_CALLBACK_TOKEN_VALUE && dispatch_thread == GetCurrentThreadId())
            token = executing;
        auto* r = find(token);
        if (token && r) stop(*r);
    }
    bool STDMETHODCALLTYPE UnregisterCallback(GameInputCallbackToken token, uint64_t timeout_us) noexcept override {
        std::unique_lock<std::mutex> lock(mutex);
        auto* r = find(token);
        if (!token || !r) return false;
        stop(*r);
        if (executing == token) {
            if (dispatch_thread == GetCurrentThreadId()) return false;
            auto done = [&] { return executing != token; };
            if (timeout_us == UINT64_MAX) changed.wait(lock, done);
            else {
                // Bound the chrono conversion for very large finite SDK timeouts.
                const auto interval = (std::min)(timeout_us, uint64_t(INT64_MAX / 2000));
                if (!changed.wait_for(lock, std::chrono::microseconds(interval), done)) return false;
            }
        }
        if (r->token == token) *r = {};
        return true;
    }
    HRESULT STDMETHODCALLTYPE CreateDispatcher(IGameInputDispatcher** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE CreateAggregateDevice(GameInputKind, IGameInputDevice** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE FindDeviceFromId(const APP_LOCAL_DEVICE_ID*, IGameInputDevice** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE FindDeviceFromObject(IUnknown*, IGameInputDevice** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE FindDeviceFromPlatformHandle(HANDLE, IGameInputDevice** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE FindDeviceFromPlatformString(LPCWSTR, IGameInputDevice** out) noexcept override { return unsupported(out); }
    HRESULT STDMETHODCALLTYPE EnableOemDeviceSupport(uint16_t, uint16_t, uint8_t, uint8_t) noexcept override { return E_NOTIMPL; }
    void STDMETHODCALLTYPE SetFocusPolicy(GameInputFocusPolicy) noexcept override {}
};
}

HRESULT WINAPI GameInputCreate(IGameInput** out) {
    if (!out) return E_POINTER;
    static const auto input = Make<Input>();
    *out = nullptr;
    return input ? input->open(out) : E_OUTOFMEMORY;
}
