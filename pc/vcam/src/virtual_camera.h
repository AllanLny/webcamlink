#pragma once

#include <windows.h>
#include <unknwn.h>

namespace wcl {

// Virtual camera attributes (mfvirtualcamera.h hides these behind
// WINAPI_PARTITION_APP, so the values are repeated here).
// {1BB79E7C-5D83-438C-94D8-E5F0DF6D3279}
static const GUID kAssociatedCameraSources = {
    0x1bb79e7c, 0x5d83, 0x438c, {0x94, 0xd8, 0xe5, 0xf0, 0xdf, 0x6d, 0x32, 0x79}};
// {F0273718-4A4D-4AC5-A15D-305EB5E90667}
static const GUID kProvideAssociatedCameraSources = {
    0xf0273718, 0x4a4d, 0x4ac5, {0xa1, 0x5d, 0x30, 0x5e, 0xb5, 0xe9, 0x06, 0x67}};
// {28A5531A-57DD-4FD5-AAA7-385ABF57D785}
static const GUID kEnableMsCameraEffects = {
    0x28a5531a, 0x57dd, 0x4fd5, {0xaa, 0xa7, 0x38, 0x5a, 0xbf, 0x57, 0xd7, 0x85}};

/// Creates the WebcamLink virtual camera media source (IMFMediaSourceEx).
HRESULT CreateVirtualCamera(IUnknown** source);

/// Creates the IMFActivate the Frame Server instantiates from our class
/// factory; it builds the media source on demand.
HRESULT CreateCameraActivate(IUnknown** activate);

/// Appends a line to %TEMP%\webcamlink-vcam.log (the DLL runs inside the
/// Frame Server, a file is the only observable output).
void LogLine(const wchar_t* message);

}  // namespace wcl
