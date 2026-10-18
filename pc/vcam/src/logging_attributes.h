#pragma once

#include <windows.h>

#include <mfapi.h>
#include <mfidl.h>
#include <propvarutil.h>
#include <wrl/client.h>
#include <wrl/implements.h>

#include "virtual_camera.h"

namespace wcl {

/// IMFAttributes that forwards everything to a real store and writes every
/// keyed access to the log. The Frame Server reads the source and stream
/// attributes through it, so a lookup it depends on shows up as a line with
/// the exact key GUID instead of a silent MF_E_ATTRIBUTENOTFOUND.
class LoggingAttributes
    : public Microsoft::WRL::RuntimeClass<Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>,
                                          IMFAttributes> {
public:
    explicit LoggingAttributes(const wchar_t* tag) : tag_(tag) {
        MFCreateAttributes(&store_, 12);
    }

    /// The wrapped store, for this DLL to read and write directly.
    IMFAttributes* Store() { return store_.Get(); }

    // IMFAttributes
    STDMETHODIMP GetItem(REFGUID key, PROPVARIANT* value) override {
        return Log(L"GetItem", key, store_->GetItem(key, value));
    }
    STDMETHODIMP GetItemType(REFGUID key, MF_ATTRIBUTE_TYPE* type) override {
        return Log(L"GetItemType", key, store_->GetItemType(key, type));
    }
    STDMETHODIMP CompareItem(REFGUID key, REFPROPVARIANT value, BOOL* result) override {
        return Log(L"CompareItem", key, store_->CompareItem(key, value, result));
    }
    STDMETHODIMP Compare(IMFAttributes* theirs, MF_ATTRIBUTES_MATCH_TYPE match,
                         BOOL* result) override {
        return store_->Compare(theirs, match, result);
    }
    STDMETHODIMP GetUINT32(REFGUID key, UINT32* value) override {
        const HRESULT hr = store_->GetUINT32(key, value);
        wchar_t detail[32]{};
        if (SUCCEEDED(hr) && value) {
            swprintf_s(detail, L" = %u", *value);
        }
        return Log(L"GetUINT32", key, hr, detail);
    }
    STDMETHODIMP GetUINT64(REFGUID key, UINT64* value) override {
        const HRESULT hr = store_->GetUINT64(key, value);
        wchar_t detail[32]{};
        if (SUCCEEDED(hr) && value) {
            swprintf_s(detail, L" = %llu", *value);
        }
        return Log(L"GetUINT64", key, hr, detail);
    }
    STDMETHODIMP GetDouble(REFGUID key, double* value) override {
        return Log(L"GetDouble", key, store_->GetDouble(key, value));
    }
    STDMETHODIMP GetGUID(REFGUID key, GUID* value) override {
        const HRESULT hr = store_->GetGUID(key, value);
        wchar_t detail[96]{};
        if (SUCCEEDED(hr) && value) {
            wchar_t found[64]{};
            StringFromGUID2(*value, found, 64);
            swprintf_s(detail, L" = %s", found);
        }
        return Log(L"GetGUID", key, hr, detail);
    }
    STDMETHODIMP GetStringLength(REFGUID key, UINT32* length) override {
        return Log(L"GetStringLength", key, store_->GetStringLength(key, length));
    }
    STDMETHODIMP GetString(REFGUID key, LPWSTR value, UINT32 size, UINT32* length) override {
        return Log(L"GetString", key, store_->GetString(key, value, size, length));
    }
    STDMETHODIMP GetAllocatedString(REFGUID key, LPWSTR* value, UINT32* length) override {
        return Log(L"GetAllocatedString", key, store_->GetAllocatedString(key, value, length));
    }
    STDMETHODIMP GetBlobSize(REFGUID key, UINT32* size) override {
        return Log(L"GetBlobSize", key, store_->GetBlobSize(key, size));
    }
    STDMETHODIMP GetBlob(REFGUID key, UINT8* buffer, UINT32 size, UINT32* blob_size) override {
        return Log(L"GetBlob", key, store_->GetBlob(key, buffer, size, blob_size));
    }
    STDMETHODIMP GetAllocatedBlob(REFGUID key, UINT8** buffer, UINT32* size) override {
        return Log(L"GetAllocatedBlob", key, store_->GetAllocatedBlob(key, buffer, size));
    }
    STDMETHODIMP GetUnknown(REFGUID key, REFIID riid, LPVOID* value) override {
        const HRESULT hr = store_->GetUnknown(key, riid, value);
        wchar_t interface_id[64]{};
        StringFromGUID2(riid, interface_id, 64);
        wchar_t detail[96]{};
        swprintf_s(detail, L" as %s", interface_id);
        return Log(L"GetUnknown", key, hr, detail);
    }
    STDMETHODIMP SetItem(REFGUID key, REFPROPVARIANT value) override {
        return Log(L"SetItem", key, store_->SetItem(key, value));
    }
    STDMETHODIMP DeleteItem(REFGUID key) override {
        return Log(L"DeleteItem", key, store_->DeleteItem(key));
    }
    STDMETHODIMP SetUINT32(REFGUID key, UINT32 value) override {
        wchar_t detail[32]{};
        swprintf_s(detail, L" = %u", value);
        return Log(L"SetUINT32", key, store_->SetUINT32(key, value), detail);
    }
    STDMETHODIMP SetUINT64(REFGUID key, UINT64 value) override {
        return Log(L"SetUINT64", key, store_->SetUINT64(key, value));
    }
    STDMETHODIMP SetDouble(REFGUID key, double value) override {
        return Log(L"SetDouble", key, store_->SetDouble(key, value));
    }
    STDMETHODIMP SetGUID(REFGUID key, REFGUID value) override {
        wchar_t stored[64]{};
        StringFromGUID2(value, stored, 64);
        wchar_t detail[96]{};
        swprintf_s(detail, L" = %s", stored);
        return Log(L"SetGUID", key, store_->SetGUID(key, value), detail);
    }
    STDMETHODIMP SetString(REFGUID key, LPCWSTR value) override {
        wchar_t detail[160]{};
        if (value) {
            swprintf_s(detail, L" = %s", value);
        }
        return Log(L"SetString", key, store_->SetString(key, value), detail);
    }
    STDMETHODIMP SetBlob(REFGUID key, const UINT8* buffer, UINT32 size) override {
        return Log(L"SetBlob", key, store_->SetBlob(key, buffer, size));
    }
    STDMETHODIMP SetUnknown(REFGUID key, IUnknown* value) override {
        wchar_t detail[64]{};
        if (value) {
            swprintf_s(detail, L" (object %p)", static_cast<const void*>(value));
        } else {
            swprintf_s(detail, L" = null");
        }
        return Log(L"SetUnknown", key, store_->SetUnknown(key, value), detail);
    }
    STDMETHODIMP DeleteAllItems() override {
        return Log(L"DeleteAllItems", GUID_NULL, store_->DeleteAllItems());
    }
    STDMETHODIMP LockStore() override { return store_->LockStore(); }
    STDMETHODIMP UnlockStore() override { return store_->UnlockStore(); }
    STDMETHODIMP GetCount(UINT32* count) override {
        const HRESULT hr = store_->GetCount(count);
        wchar_t detail[32]{};
        if (SUCCEEDED(hr) && count) {
            swprintf_s(detail, L" = %u", *count);
        }
        return Log(L"GetCount", GUID_NULL, hr, detail);
    }
    STDMETHODIMP GetItemByIndex(UINT32 index, GUID* key, PROPVARIANT* value) override {
        const HRESULT hr = store_->GetItemByIndex(index, key, value);
        wchar_t detail[96]{};
        if (SUCCEEDED(hr) && key) {
            wchar_t found[64]{};
            StringFromGUID2(*key, found, 64);
            swprintf_s(detail, L" [%u] = %s", index, found);
        }
        return Log(L"GetItemByIndex", GUID_NULL, hr, detail);
    }
    STDMETHODIMP CopyAllItems(IMFAttributes* destination) override {
        // Every item leaving the store is logged: a reader that copies first
        // and inspects the copy is the one case the per-call log cannot see.
        UINT32 count = 0;
        if (SUCCEEDED(store_->GetCount(&count))) {
            for (UINT32 index = 0; index < count; ++index) {
                GUID key{};
                PROPVARIANT value{};
                if (SUCCEEDED(store_->GetItemByIndex(index, &key, &value))) {
                    Log(L"CopyAllItems item", key, S_OK);
                    PropVariantClear(&value);
                }
            }
        }
        return Log(L"CopyAllItems", GUID_NULL, store_->CopyAllItems(destination));
    }

private:
    HRESULT Log(const wchar_t* method, REFGUID key, HRESULT hr, const wchar_t* detail = L"") {
        wchar_t guid[64]{};
        StringFromGUID2(key, guid, 64);
        wchar_t line[320];
        swprintf_s(line, L"%s::%s(%s) -> 0x%08lX%s", tag_, method, guid,
                   static_cast<unsigned long>(hr), detail);
        LogLine(line);
        return hr;
    }

    const wchar_t* tag_;
    Microsoft::WRL::ComPtr<IMFAttributes> store_;
};

}  // namespace wcl
