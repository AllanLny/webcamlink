// Registers the WebcamLink virtual camera: COM (HKLM) + the virtual camera
// device through MFCreateVirtualCamera (exported by mfsensorgroup.dll).
// Requires an elevated process.
#include <windows.h>

#include <mfapi.h>
#include <mfidl.h>
#include <mfvirtualcamera.h>

#include <cstdio>
#include <string>

#include "../include/wcl_shm.h"

namespace {

// KSCATEGORY_VIDEO_CAMERA, so the camera shows up for Windows Camera, Teams...
const GUID kVideoCameraCategory = {
    0xe5323777, 0xf976, 0x4f5b, {0x9b, 0x55, 0xb9, 0x46, 0x99, 0xc4, 0x6e, 0x44}};

const wchar_t* kClsidString = L"{B7C0E1F2-3A4D-4E5F-9A6B-1C2D3E4F5A6B}";
const wchar_t* kFriendlyName = L"WebcamLink";

using CreateVirtualCameraFn = HRESULT(WINAPI*)(MFVirtualCameraType, MFVirtualCameraLifetime,
                                               MFVirtualCameraAccess, LPCWSTR, LPCWSTR,
                                               const GUID*, ULONG, IMFVirtualCamera**);

bool IsElevated() {
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
        return false;
    }
    TOKEN_ELEVATION elevation{};
    DWORD size = sizeof(elevation);
    const bool elevated =
        GetTokenInformation(token, TokenElevation, &elevation, size, &size) && elevation.TokenIsElevated;
    CloseHandle(token);
    return elevated;
}

std::wstring ModuleDirectory() {
    wchar_t path[MAX_PATH]{};
    GetModuleFileNameW(nullptr, path, MAX_PATH);
    std::wstring full(path);
    const size_t slash = full.find_last_of(L'\\');
    return slash == std::wstring::npos ? full : full.substr(0, slash);
}

CreateVirtualCameraFn LoadCreateVirtualCamera() {
    HMODULE module = LoadLibraryW(L"mfsensorgroup.dll");
    if (!module) {
        return nullptr;
    }
    return reinterpret_cast<CreateVirtualCameraFn>(
        GetProcAddress(module, "MFCreateVirtualCamera"));
}

bool RegisterComServer(const std::wstring& dll_path) {
    HKEY key = nullptr;
    const std::wstring clsid_key = std::wstring(L"SOFTWARE\\Classes\\CLSID\\") + kClsidString;
    if (RegCreateKeyExW(HKEY_LOCAL_MACHINE, clsid_key.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr,
                        &key, nullptr) != ERROR_SUCCESS) {
        return false;
    }
    RegSetValueExW(key, nullptr, 0, REG_SZ, reinterpret_cast<const BYTE*>(kFriendlyName),
                   static_cast<DWORD>((wcslen(kFriendlyName) + 1) * sizeof(wchar_t)));
    RegCloseKey(key);

    const std::wstring inproc_key = clsid_key + L"\\InprocServer32";
    if (RegCreateKeyExW(HKEY_LOCAL_MACHINE, inproc_key.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr,
                        &key, nullptr) != ERROR_SUCCESS) {
        return false;
    }
    RegSetValueExW(key, nullptr, 0, REG_SZ, reinterpret_cast<const BYTE*>(dll_path.c_str()),
                   static_cast<DWORD>((dll_path.size() + 1) * sizeof(wchar_t)));
    const wchar_t* threading = L"Both";
    RegSetValueExW(key, L"ThreadingModel", 0, REG_SZ, reinterpret_cast<const BYTE*>(threading),
                   static_cast<DWORD>((wcslen(threading) + 1) * sizeof(wchar_t)));
    RegCloseKey(key);
    return true;
}

void UnregisterComServer() {
    const std::wstring clsid_key = std::wstring(L"SOFTWARE\\Classes\\CLSID\\") + kClsidString;
    RegDeleteTreeW(HKEY_LOCAL_MACHINE, clsid_key.c_str());
}

int Register(bool enable) {
    if (!IsElevated()) {
        wprintf(L"Cette commande doit être lancée en administrateur.\n");
        return 5;
    }

    CreateVirtualCameraFn create = LoadCreateVirtualCamera();
    if (!create) {
        wprintf(L"MFCreateVirtualCamera introuvable (mfsensorgroup.dll).\n");
        return 1;
    }

    IMFVirtualCamera* camera = nullptr;
    HRESULT hr = create(MFVirtualCameraType_SoftwareCameraSource, MFVirtualCameraLifetime_System,
                        MFVirtualCameraAccess_AllUsers, kFriendlyName, kClsidString,
                        &kVideoCameraCategory, 1, &camera);
    if (FAILED(hr) || !camera) {
        wprintf(L"MFCreateVirtualCamera a échoué : 0x%08lX\n", static_cast<unsigned long>(hr));
        return 1;
    }

    if (!enable) {
        hr = camera->Remove();
        wprintf(L"Caméra virtuelle supprimée : 0x%08lX\n", static_cast<unsigned long>(hr));
        camera->Release();
        UnregisterComServer();
        return SUCCEEDED(hr) ? 0 : 1;
    }

    const std::wstring dll_path = ModuleDirectory() + L"\\wcl-vcam.dll";
    if (!RegisterComServer(dll_path)) {
        wprintf(L"Enregistrement COM impossible (HKLM).\n");
        camera->Release();
        return 1;
    }

    hr = camera->Start(nullptr);
    wprintf(L"MFCreateVirtualCamera OK, Start : 0x%08lX\n", static_cast<unsigned long>(hr));
    if (SUCCEEDED(hr)) {
        wprintf(L"Caméra virtuelle \"%s\" enregistrée (source %s, DLL %s).\n", kFriendlyName,
                kClsidString, dll_path.c_str());
    }
    camera->Release();
    return SUCCEEDED(hr) ? 0 : 1;
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    const bool enable = !(argc > 1 && (wcscmp(argv[1], L"--unregister") == 0 ||
                                       wcscmp(argv[1], L"-u") == 0));

    // MFCreateVirtualCamera returns CO_E_NOTINITIALIZED without these.
    const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const HRESULT mf = MFStartup(MF_VERSION);
    if (FAILED(mf)) {
        wprintf(L"MFStartup a échoué : 0x%08lX\n", static_cast<unsigned long>(mf));
    }

    const int result = Register(enable);

    if (SUCCEEDED(mf)) {
        MFShutdown();
    }
    if (SUCCEEDED(com)) {
        CoUninitialize();
    }
    return result;
}
