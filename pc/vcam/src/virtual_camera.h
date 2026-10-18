#pragma once

#include <windows.h>
#include <unknwn.h>

namespace wcl {

/// Creates the WebcamLink virtual camera media source (IMFMediaSourceEx).
HRESULT CreateVirtualCamera(IUnknown** source);

}  // namespace wcl
