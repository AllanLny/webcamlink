// COM entry points for the WebcamLink virtual camera DLL, loaded by the
// Windows Frame Server (session 0, LOCAL SERVICE).
#include <windows.h>

#include <mfapi.h>
#include <wrl/client.h>
#include <wrl/implements.h>

#include <atomic>
#include <cstdio>
#include <string>

#include "../include/wcl_shm.h"
#include "config.h"
#include "virtual_camera.h"

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::ClassicCom;
using Microsoft::WRL::Make;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;

namespace {

std::atomic<LONG> g_server_locks{0};
std::atomic<bool> g_mf_started{false};

/// Opens the log file. The DLL runs inside the Frame Server (session 0), so a
/// file is the only observable output — and it has to be readable from the user
/// session, hence ProgramData first and the process temp directory as fallback.
FILE* OpenLog() {
    const std::wstring directory = wcl::WebcamLinkDataDirectory();
    if (!directory.empty()) {
        CreateDirectoryW(directory.c_str(), nullptr);
        const std::wstring path = directory + L"\\vcam.log";
        FILE* file = nullptr;
        if (_wfopen_s(&file, path.c_str(), L"a") == 0 && file) {
            return file;
        }
    }

    wchar_t fallback[MAX_PATH]{};
    if (GetTempPathW(MAX_PATH, fallback) == 0) {
        return nullptr;
    }
    wcscat_s(fallback, L"webcamlink-vcam.log");
    FILE* file = nullptr;
    if (_wfopen_s(&file, fallback, L"a") != 0 || !file) {
        return nullptr;
    }
    return file;
}

void Log(const wchar_t* message) {
    FILE* file = OpenLog();
    if (!file) {
        return;
    }
    SYSTEMTIME now{};
    GetLocalTime(&now);
    // The process id tells the registering process (session 3) apart from the
    // Frame Server (session 0), both of which log to the same file.
    fwprintf(file, L"[%02d:%02d:%02d.%03d %lu] %s\n", now.wHour, now.wMinute, now.wSecond,
             now.wMilliseconds, GetCurrentProcessId(), message);
    fclose(file);
}

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
        wchar_t guid[64]{};
        StringFromGUID2(riid, guid, 64);
        wchar_t line[192];
        swprintf_s(line, L"CreateInstance(riid=%s)", guid);
        Log(line);

        EnsureMediaFoundation();

        ComPtr<IUnknown> activate;
        HRESULT hr = wcl::CreateCameraActivate(&activate);
        if (FAILED(hr)) {
            Log(L"CreateCameraActivate a échoué");
            return hr;
        }
        hr = activate->QueryInterface(riid, object);
        swprintf_s(line, L"CreateInstance(riid=%s) -> 0x%08lX", guid,
                   static_cast<unsigned long>(hr));
        Log(line);
        return hr;
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

namespace wcl {

void LogLine(const wchar_t* message) { Log(message); }

}  // namespace wcl

STDAPI DllGetClassObject(REFCLSID clsid, REFIID riid, void** object) {
    if (!object) {
        return E_POINTER;
    }
    *object = nullptr;
    wchar_t clsid_text[64]{};
    StringFromGUID2(clsid, clsid_text, 64);
    wchar_t line[160];
    swprintf_s(line, L"DllGetClassObject(%s)", clsid_text);
    Log(line);
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
