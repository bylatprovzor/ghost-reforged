// Linked only by the native audio tests; no stream is started or made audible.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <wrl/client.h>

using Microsoft::WRL::ComPtr;
namespace {
struct Fixture {
    ComPtr<IAudioClient> client;
    ComPtr<ISimpleAudioVolume> volume;
};
}

extern "C" void* ghost_audio_fixture_open(int muted) noexcept {
    if (FAILED(CoInitializeEx(nullptr, COINIT_MULTITHREADED))) return nullptr;
    auto* fixture = new (std::nothrow) Fixture;
    if (fixture) {
        ComPtr<IMMDeviceEnumerator> devices;
        ComPtr<IMMDevice> device;
        WAVEFORMATEX* format = nullptr;
        GUID id;
        if (SUCCEEDED(CoCreateGuid(&id)) &&
            SUCCEEDED(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, IID_PPV_ARGS(&devices))) &&
            SUCCEEDED(devices->GetDefaultAudioEndpoint(eRender, eConsole, &device)) &&
            SUCCEEDED(device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, &fixture->client)) &&
            SUCCEEDED(fixture->client->GetMixFormat(&format))) {
            const auto result = fixture->client->Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, 1000000, 0, format, &id);
            CoTaskMemFree(format);
            if (SUCCEEDED(result) && SUCCEEDED(fixture->client->GetService(IID_PPV_ARGS(&fixture->volume)))) {
                fixture->volume->SetMute(muted != 0, nullptr);
                return fixture;
            }
        }
        delete fixture;
    }
    CoUninitialize();
    return nullptr;
}
extern "C" int ghost_audio_fixture_muted(void* raw) noexcept {
    BOOL muted = FALSE;
    if (FAILED(static_cast<Fixture*>(raw)->volume->GetMute(&muted))) return -1;
    return muted ? 1 : 0;
}
extern "C" void ghost_audio_fixture_close(void* raw) noexcept {
    delete static_cast<Fixture*>(raw);
    CoUninitialize();
}
