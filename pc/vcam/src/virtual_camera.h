#pragma once

#include <windows.h>
#include <unknwn.h>

namespace wcl {

/// Creates the WebcamLink virtual camera media source (IMFMediaSourceEx).
HRESULT CreateVirtualCamera(IUnknown** source);

/// Creates the IMFActivate the Frame Server instantiates from our class
/// factory; it builds the media source on demand.
HRESULT CreateCameraActivate(IUnknown** activate);

/// Appends a line to %TEMP%\webcamlink-vcam.log (the DLL runs inside the
/// Frame Server, a file is the only observable output).
void LogLine(const wchar_t* message);

}  // namespace wcl
