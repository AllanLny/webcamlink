#pragma once

#include <windows.h>
#include <unknwn.h>

namespace wcl {

/// Creates the WebcamLink virtual camera media source (IMFMediaSourceEx).
HRESULT CreateVirtualCamera(IUnknown** source);

/// Creates the IMFActivate the Frame Server instantiates from our class
/// factory; it builds the media source on demand.
HRESULT CreateCameraActivate(IUnknown** activate);

}  // namespace wcl
