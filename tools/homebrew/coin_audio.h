#pragma once
#include <windows.h>
#include <xaudio2.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <wrl/client.h>

enum class CoinSide { Left, Right };

class CoinAudio {
    static constexpr unsigned rate = 48000;
    struct Tone {
        IXAudio2SourceVoice* voice = nullptr;
        std::array<int16_t, rate * 120 / 1000> samples{};
    };
    std::array<Tone, 2> tones{};
    bool owns_com = false;
    HMODULE library = nullptr;
    Microsoft::WRL::ComPtr<IXAudio2> engine;
    IXAudio2MasteringVoice* master = nullptr;

public:
    CoinAudio() = default;
    CoinAudio(const CoinAudio&) = delete;
    CoinAudio& operator=(const CoinAudio&) = delete;

    ~CoinAudio() {
        for (auto& tone : tones) {
            if (tone.voice) tone.voice->DestroyVoice();
        }
        if (master) master->DestroyVoice();
        engine.Reset();
        if (library) FreeLibrary(library);
        if (owns_com) CoUninitialize();
    }

    HRESULT init() {
        HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        owns_com = SUCCEEDED(hr);
        if (FAILED(hr) && hr != RPC_E_CHANGED_MODE) return hr;
        library = LoadLibraryExW(L"XAudio2_9.dll", nullptr, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (!library) return HRESULT_FROM_WIN32(GetLastError());
        // Use the factory ordinal recorded in the console import table.
        using Create = HRESULT (WINAPI*)(IXAudio2**, UINT32, XAUDIO2_PROCESSOR);
        auto create = reinterpret_cast<Create>(GetProcAddress(library, MAKEINTRESOURCEA(1)));
        if (!create) return HRESULT_FROM_WIN32(GetLastError());
        hr = create(engine.GetAddressOf(), 0, XAUDIO2_DEFAULT_PROCESSOR);
        if (FAILED(hr)) return hr;
        hr = engine->CreateMasteringVoice(&master, 2);
        if (FAILED(hr)) return hr;
        WAVEFORMATEX format{WAVE_FORMAT_PCM, 1, rate, rate * 2, 2, 16, 0};
        const float gains[][2] = {{1.f, 0.25f}, {0.25f, 1.f}};
        const double frequency[] = {440.0, 880.0};
        for (size_t side = 0; side < tones.size(); ++side) {
            auto& tone = tones[side];
            hr = engine->CreateSourceVoice(&tone.voice, &format);
            if (FAILED(hr)) return hr;
            hr = tone.voice->SetOutputMatrix(master, 1, 2, gains[side]);
            if (FAILED(hr)) return hr;
            const double phase_step = 2 * 3.141592653589793 * frequency[side] / rate;
            for (size_t i = 0; i < tone.samples.size(); ++i) {
                const size_t edge = (std::min)(i, tone.samples.size() - 1 - i);
                const double envelope = (std::min)(1.0, edge / 480.0); // 10 ms attack and release
                tone.samples[i] = static_cast<int16_t>(6000 * envelope * std::sin(i * phase_step));
            }
            hr = tone.voice->Start();
            if (FAILED(hr)) return hr;
        }
        return S_OK;
    }

    HRESULT pickup(CoinSide side) {
        const auto& tone = tones[static_cast<size_t>(side)];
        XAUDIO2_BUFFER buffer{};
        buffer.Flags = XAUDIO2_END_OF_STREAM;
        buffer.AudioBytes = sizeof(tone.samples);
        buffer.pAudioData = reinterpret_cast<const BYTE*>(tone.samples.data());
        return tone.voice->SubmitSourceBuffer(&buffer);
    }
};
