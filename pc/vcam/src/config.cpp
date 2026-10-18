#include "config.h"

#include <cstdio>

#include "virtual_camera.h"

namespace wcl {
namespace {

UINT32 ReadInt(const std::wstring& path, const wchar_t* key, UINT32 fallback, UINT32 minimum,
               UINT32 maximum) {
    if (path.empty()) {
        return fallback;
    }
    const UINT32 value = GetPrivateProfileIntW(L"video", key, fallback, path.c_str());
    if (value < minimum) {
        return minimum;
    }
    return value > maximum ? maximum : value;
}

}  // namespace

std::wstring WebcamLinkDataDirectory() {
    wchar_t programdata[MAX_PATH]{};
    if (GetEnvironmentVariableW(L"ProgramData", programdata, MAX_PATH) == 0) {
        wchar_t windows[MAX_PATH]{};
        if (GetWindowsDirectoryW(windows, MAX_PATH) == 0) {
            return {};
        }
        wchar_t* slash = wcsrchr(windows, L'\\');
        if (slash) {
            *slash = L'\0';
        }
        if (swprintf_s(programdata, L"%s\\ProgramData", windows) <= 0) {
            return {};
        }
    }
    return std::wstring(programdata) + L"\\WebcamLink";
}

VideoConfig LoadVideoConfig() {
    VideoConfig config;
    const std::wstring directory = WebcamLinkDataDirectory();
    const std::wstring path = directory.empty() ? std::wstring() : directory + L"\\config.ini";

    config.width = ReadInt(path, L"width", config.width, 160, 3840);
    config.height = ReadInt(path, L"height", config.height, 120, 2160);
    config.fps = ReadInt(path, L"fps", config.fps, 1, 60);
    config.bitrate_kbps = ReadInt(path, L"bitrate_kbps", config.bitrate_kbps, 500, 100'000);

    wchar_t line[224];
    swprintf_s(line, L"config: %ux%u @ %u fps, %u kbps (%s)", config.width, config.height,
               config.fps, config.bitrate_kbps, path.empty() ? L"défauts" : path.c_str());
    LogLine(line);
    return config;
}

}  // namespace wcl
