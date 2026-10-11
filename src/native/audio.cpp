#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mmdeviceapi.h>
#include <audiopolicy.h>
#include <wrl/client.h>
#include <atomic>
#include <mutex>
#include <string>
#include <vector>
#include <algorithm>

using Microsoft::WRL::ComPtr;

namespace {
struct Session {
    std::wstring id;
    ComPtr<IAudioSessionControl2> control;
    ComPtr<ISimpleAudioVolume> volume;
    BOOL muted;
};

class Sessions final : public IAudioSessionNotification {
    std::atomic<ULONG> refs{1};
    std::mutex mutex;
    HANDLE process;
    DWORD pid;
    bool active = true;
    std::vector<Session> saved;
public:
    explicit Sessions(DWORD target) : process(OpenProcess(SYNCHRONIZE, FALSE, target)), pid(target) {}
    ~Sessions() { if (process) CloseHandle(process); }
    bool valid() const { return process != nullptr; }
    ULONG STDMETHODCALLTYPE AddRef() override { return ++refs; }
    ULONG STDMETHODCALLTYPE Release() override {
        const auto remaining = --refs;
        if (!remaining) delete this;
        return remaining;
    }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
        if (!result) return E_POINTER;
        *result = nullptr;
        if (iid != __uuidof(IUnknown) && iid != __uuidof(IAudioSessionNotification)) return E_NOINTERFACE;
        *result = static_cast<IAudioSessionNotification*>(this);
        AddRef();
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnSessionCreated(IAudioSessionControl* session) override {
        try { capture(session); } catch (...) { return E_OUTOFMEMORY; }
        return S_OK;
    }
    void capture(IAudioSessionControl* session) {
        std::lock_guard<std::mutex> lock(mutex);
        if (!active || WaitForSingleObject(process, 0) != WAIT_TIMEOUT) return;
        ComPtr<IAudioSessionControl2> control;
        DWORD owner = 0;
        // S_FALSE can denote a shared cross-process session. Never mute it.
        if (FAILED(session->QueryInterface(IID_PPV_ARGS(&control))) ||
            control->GetProcessId(&owner) != S_OK || owner != pid) return;
        LPWSTR raw = nullptr;
        if (FAILED(control->GetSessionInstanceIdentifier(&raw))) return;
        std::wstring id;
        try { id = raw; } catch (...) { CoTaskMemFree(raw); throw; }
        CoTaskMemFree(raw);
        const auto found = std::find_if(saved.begin(), saved.end(), [&](const Session& item) { return item.id == id; });
        if (found != saved.end()) { found->volume->SetMute(TRUE, nullptr); return; }
        ComPtr<ISimpleAudioVolume> volume;
        BOOL muted = FALSE;
        if (FAILED(session->QueryInterface(IID_PPV_ARGS(&volume))) || FAILED(volume->GetMute(&muted))) return;
        // Save before the mutation so allocation failure cannot lose restoration state.
        saved.push_back({std::move(id), control, volume, muted});
        volume->SetMute(TRUE, nullptr);
    }
    void restore() {
        std::lock_guard<std::mutex> lock(mutex);
        active = false;
        for (auto& session : saved) session.volume->SetMute(session.muted, nullptr);
        saved.clear();
    }
};

struct Endpoint {
    std::wstring id;
    ComPtr<IAudioSessionManager2> manager;
};

class Audio {
    ComPtr<IMMDeviceEnumerator> devices;
    ComPtr<Sessions> sessions;
    std::vector<Endpoint> endpoints;
public:
    explicit Audio(DWORD pid) {
        sessions.Attach(new Sessions(pid));
        if (!sessions->valid()) throw E_FAIL;
        const auto result = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, IID_PPV_ARGS(&devices));
        if (FAILED(result)) throw result;
    }
    ~Audio() {
        sessions->restore();
        for (auto& endpoint : endpoints) endpoint.manager->UnregisterSessionNotification(sessions.Get());
    }
    void refresh() {
        ComPtr<IMMDeviceCollection> collection;
        if (FAILED(devices->EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE, &collection))) return;
        UINT count = 0;
        if (FAILED(collection->GetCount(&count))) return;
        std::vector<std::wstring> present;
        for (UINT index = 0; index < count; ++index) {
            ComPtr<IMMDevice> device;
            LPWSTR raw = nullptr;
            if (FAILED(collection->Item(index, &device)) || FAILED(device->GetId(&raw))) continue;
            std::wstring id;
            try { id = raw; } catch (...) { CoTaskMemFree(raw); throw; }
            CoTaskMemFree(raw);
            present.push_back(id);
            auto found = std::find_if(endpoints.begin(), endpoints.end(), [&](const Endpoint& endpoint) { return endpoint.id == id; });
            if (found == endpoints.end()) {
                ComPtr<IAudioSessionManager2> manager;
                if (FAILED(device->Activate(__uuidof(IAudioSessionManager2), CLSCTX_ALL, nullptr, &manager))) continue;
                endpoints.push_back({id, manager});
                if (FAILED(manager->RegisterSessionNotification(sessions.Get()))) { endpoints.pop_back(); continue; }
                found = endpoints.end() - 1;
            }
            ComPtr<IAudioSessionEnumerator> list;
            int total = 0;
            if (FAILED(found->manager->GetSessionEnumerator(&list)) || FAILED(list->GetCount(&total))) {
                found->manager->UnregisterSessionNotification(sessions.Get());
                endpoints.erase(found);
                continue;
            }
            for (int item = 0; item < total; ++item) {
                ComPtr<IAudioSessionControl> session;
                if (SUCCEEDED(list->GetSession(item, &session))) sessions->capture(session.Get());
            }
        }
        for (auto iterator = endpoints.begin(); iterator != endpoints.end();) {
            if (std::find(present.begin(), present.end(), iterator->id) == present.end()) {
                iterator->manager->UnregisterSessionNotification(sessions.Get());
                iterator = endpoints.erase(iterator);
            } else { ++iterator; }
        }
    }
};
}

// All calls belong to the same dedicated MTA worker. No COM pointer crosses to Rust.
extern "C" void* ghost_audio_open(unsigned long pid) noexcept {
    if (FAILED(CoInitializeEx(nullptr, COINIT_MULTITHREADED))) return nullptr;
    Audio* audio = nullptr;
    try { audio = new Audio(pid); audio->refresh(); return audio; }
    catch (...) { delete audio; CoUninitialize(); return nullptr; }
}
extern "C" void ghost_audio_refresh(void* context) noexcept {
    try { static_cast<Audio*>(context)->refresh(); } catch (...) {}
}
extern "C" void ghost_audio_close(void* context) noexcept {
    delete static_cast<Audio*>(context);
    CoUninitialize();
}
