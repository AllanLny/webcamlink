// COM entry points for the WebcamLink virtual camera DLL, loaded by the
// Windows Frame Server (session 0, LOCAL SERVICE).
#include <windows.h>

#include <mfapi.h>
#include <wrl/client.h>
#include <wrl/implements.h>

#include <atomic>

#include "../include/wcl_shm.h"
#include "virtual_camera.h"

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::ClassicCom;
using Microsoft::WRL::Make;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;

namespace {

std::atomic<LONG> g_server_locks{0};
std::atomic<bool> g_mf_started{false};

void EnsureMediaFoundation() {
    if (g_mf_started.exchange(true)) {
        return;
    }
    if (FAILED(MFStartup(MF_VERSION))) {
        g_mf_started = false;
    }
}

class VirtualCameraFactory : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IClassFactory> {
public:
    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** object) override {
        if (!object) {
            return E_POINTER;
        }
        *object = nullptr;
        if (outer) {
            return CLASS_E_NOAGGREGATION;
        }
        EnsureMediaFoundation();

        ComPtr<IUnknown> source;
        HRESULT hr = wcl::CreateVirtualCamera(&source);
        if (FAILED(hr)) {
            return hr;
        }
        return source->QueryInterface(riid, object);
    }

    STDMETHODIMP LockServer(BOOL lock) override {
        if (lock) {
            g_server_locks.fetch_add(1);
        } else {
            g_server_locks.fetch_sub(1);
        }
        return S_OK;
    }
};

}  // namespace

STDAPI DllGetClassObject(REFCLSID clsid, REFIID riid, void** object) {
    if (!object) {
        return E_POINTER;
    }
    *object = nullptr;
    if (!IsEqualCLSID(clsid, CLSID_WebcamLinkVirtualCamera)) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    auto factory = Make<VirtualCameraFactory>();
    if (!factory) {
        return E_OUTOFMEMORY;
    }
    return factory->QueryInterface(riid, object);
}

STDAPI DllCanUnloadNow() {
    return g_server_locks.load() == 0 ? S_OK : S_FALSE;
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(instance);
    }
    return TRUE;
}
