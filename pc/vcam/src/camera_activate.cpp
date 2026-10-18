// IMFActivate wrapper: the object the Frame Server instantiates from our class
// factory. It creates the media source on demand, the standard Media
// Foundation device-source pattern.
#include <windows.h>

#include <mfapi.h>
#include <mfidl.h>
#include <wrl/client.h>
#include <wrl/implements.h>

#include <cstdio>

#include "virtual_camera.h"

using Microsoft::WRL::ClassicCom;
using Microsoft::WRL::ComPtr;
using Microsoft::WRL::Make;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;

namespace {

class CameraActivate : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IMFActivate> {
public:
    CameraActivate() {
        MFCreateAttributes(&store_, 8);
        // The Frame Server reads the associated-camera keys before anything
        // else and gives up with MF_E_ATTRIBUTENOTFOUND when they are missing.
        IMFCollection* empty = nullptr;
        if (SUCCEEDED(MFCreateCollection(&empty)) && empty) {
            store_->SetUnknown(wcl::kAssociatedCameraSources, empty);
            empty->Release();
        }
        store_->SetUINT32(wcl::kProvideAssociatedCameraSources, TRUE);
        store_->SetUINT32(wcl::kEnableMsCameraEffects, FALSE);
        store_->SetString(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, L"WebcamLink");
    }

    // IMFActivate
    STDMETHODIMP ActivateObject(REFIID riid, void** object) override {
        if (!object) {
            return E_POINTER;
        }
        *object = nullptr;
        wchar_t guid[64]{};
        StringFromGUID2(riid, guid, 64);
        wchar_t line[192];
        swprintf_s(line, L"ActivateObject(riid=%s)", guid);
        wcl::LogLine(line);

        if (!source_) {
            ComPtr<IUnknown> created;
            HRESULT hr = wcl::CreateVirtualCamera(&created);
            if (FAILED(hr)) {
                wcl::LogLine(L"CreateVirtualCamera a échoué");
                return hr;
            }
            source_ = created;
        }
        HRESULT hr = source_->QueryInterface(riid, object);
        swprintf_s(line, L"ActivateObject(riid=%s) -> 0x%08lX", guid,
                   static_cast<unsigned long>(hr));
        wcl::LogLine(line);
        return hr;
    }

    STDMETHODIMP ShutdownObject() override {
        if (source_) {
            ComPtr<IMFMediaSource> media_source;
            if (SUCCEEDED(source_.As(&media_source))) {
                media_source->Shutdown();
            }
        }
        return S_OK;
    }

    STDMETHODIMP DetachObject() override {
        source_.Reset();
        return S_OK;
    }

    // IMFAttributes — delegated to a real attribute store.
    STDMETHODIMP GetItem(REFGUID key, PROPVARIANT* value) override {
        return store_->GetItem(key, value);
    }
    STDMETHODIMP GetItemType(REFGUID key, MF_ATTRIBUTE_TYPE* type) override {
        return store_->GetItemType(key, type);
    }
    STDMETHODIMP CompareItem(REFGUID key, REFPROPVARIANT value, BOOL* result) override {
        return store_->CompareItem(key, value, result);
    }
    STDMETHODIMP Compare(IMFAttributes* theirs, MF_ATTRIBUTES_MATCH_TYPE match,
                         BOOL* result) override {
        return store_->Compare(theirs, match, result);
    }
    STDMETHODIMP GetUINT32(REFGUID key, UINT32* value) override {
        HRESULT hr = store_->GetUINT32(key, value);
        if (FAILED(hr)) {
            wchar_t guid[64]{};
            StringFromGUID2(key, guid, 64);
            wchar_t line[192];
            swprintf_s(line, L"activate::GetUINT32(%s) -> 0x%08lX", guid,
                       static_cast<unsigned long>(hr));
            wcl::LogLine(line);
        }
        return hr;
    }
    STDMETHODIMP GetUINT64(REFGUID key, UINT64* value) override {
        return store_->GetUINT64(key, value);
    }
    STDMETHODIMP GetDouble(REFGUID key, double* value) override {
        return store_->GetDouble(key, value);
    }
    STDMETHODIMP GetGUID(REFGUID key, GUID* value) override {
        HRESULT hr = store_->GetGUID(key, value);
        if (FAILED(hr)) {
            wchar_t guid[64]{};
            StringFromGUID2(key, guid, 64);
            wchar_t line[192];
            swprintf_s(line, L"activate::GetGUID(%s) -> 0x%08lX", guid,
                       static_cast<unsigned long>(hr));
            wcl::LogLine(line);
        }
        return hr;
    }
    STDMETHODIMP GetStringLength(REFGUID key, UINT32* length) override {
        return store_->GetStringLength(key, length);
    }
    STDMETHODIMP GetString(REFGUID key, LPWSTR value, UINT32 size, UINT32* length) override {
        return store_->GetString(key, value, size, length);
    }
    STDMETHODIMP GetAllocatedString(REFGUID key, LPWSTR* value, UINT32* length) override {
        return store_->GetAllocatedString(key, value, length);
    }
    STDMETHODIMP GetBlobSize(REFGUID key, UINT32* size) override {
        return store_->GetBlobSize(key, size);
    }
    STDMETHODIMP GetBlob(REFGUID key, UINT8* buffer, UINT32 size, UINT32* blob_size) override {
        return store_->GetBlob(key, buffer, size, blob_size);
    }
    STDMETHODIMP GetAllocatedBlob(REFGUID key, UINT8** buffer, UINT32* size) override {
        return store_->GetAllocatedBlob(key, buffer, size);
    }
    STDMETHODIMP GetUnknown(REFGUID key, REFIID riid, LPVOID* value) override {
        HRESULT hr = store_->GetUnknown(key, riid, value);
        if (FAILED(hr)) {
            wchar_t guid[64]{};
            StringFromGUID2(key, guid, 64);
            wchar_t line[192];
            swprintf_s(line, L"activate::GetUnknown(%s) -> 0x%08lX", guid,
                       static_cast<unsigned long>(hr));
            wcl::LogLine(line);
        }
        return hr;
    }
    STDMETHODIMP SetItem(REFGUID key, REFPROPVARIANT value) override {
        return store_->SetItem(key, value);
    }
    STDMETHODIMP DeleteItem(REFGUID key) override { return store_->DeleteItem(key); }
    STDMETHODIMP DeleteAllItems() override { return store_->DeleteAllItems(); }
    STDMETHODIMP SetUINT32(REFGUID key, UINT32 value) override {
        return store_->SetUINT32(key, value);
    }
    STDMETHODIMP SetUINT64(REFGUID key, UINT64 value) override {
        return store_->SetUINT64(key, value);
    }
    STDMETHODIMP SetDouble(REFGUID key, double value) override {
        return store_->SetDouble(key, value);
    }
    STDMETHODIMP SetGUID(REFGUID key, REFGUID value) override {
        return store_->SetGUID(key, value);
    }
    STDMETHODIMP SetString(REFGUID key, LPCWSTR value) override {
        return store_->SetString(key, value);
    }
    STDMETHODIMP SetBlob(REFGUID key, const UINT8* buffer, UINT32 size) override {
        return store_->SetBlob(key, buffer, size);
    }
    STDMETHODIMP SetUnknown(REFGUID key, IUnknown* value) override {
        return store_->SetUnknown(key, value);
    }
    STDMETHODIMP LockStore() override { return store_->LockStore(); }
    STDMETHODIMP UnlockStore() override { return store_->UnlockStore(); }
    STDMETHODIMP GetCount(UINT32* count) override { return store_->GetCount(count); }
    STDMETHODIMP GetItemByIndex(UINT32 index, GUID* key, PROPVARIANT* value) override {
        return store_->GetItemByIndex(index, key, value);
    }
    STDMETHODIMP CopyAllItems(IMFAttributes* destination) override {
        return store_->CopyAllItems(destination);
    }

private:
    ComPtr<IMFAttributes> store_;
    ComPtr<IUnknown> source_;
};

}  // namespace

namespace wcl {

HRESULT CreateCameraActivate(IUnknown** object) {
    if (!object) {
        return E_POINTER;
    }
    auto activate = Make<CameraActivate>();
    if (!activate) {
        return E_OUTOFMEMORY;
    }
    return activate.CopyTo(object);
}

}  // namespace wcl
