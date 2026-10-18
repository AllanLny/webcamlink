#pragma once

#include <windows.h>

#include <string>

namespace wcl {

/// Video format the virtual camera announces and requests from the agent.
struct VideoConfig {
    UINT32 width = 1920;
    UINT32 height = 1080;
    UINT32 fps = 30;
    UINT32 bitrate_kbps = 20'000;
};

/// `%ProgramData%\WebcamLink` — created by the registration tool, readable by
/// the user session and writable by the Frame Server account. Rebuilt from the
/// Windows directory for services whose environment lacks `ProgramData`.
/// Empty when it cannot be determined.
std::wstring WebcamLinkDataDirectory();

/// Reads `%ProgramData%\WebcamLink\config.ini` (`[video]` section), clamped to
/// what the shared memory and the pipeline support. Defaults on any failure.
VideoConfig LoadVideoConfig();

}  // namespace wcl
