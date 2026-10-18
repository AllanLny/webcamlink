cmake_minimum_required(VERSION 3.20)
project(wcl-vcam LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_library(wcl-vcam SHARED
    src/camera_activate.cpp
    src/dllmain.cpp
    src/shm_reader.cpp
    src/virtual_camera.cpp
)

target_include_directories(wcl-vcam PRIVATE include)
target_compile_definitions(wcl-vcam PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)

# COM entry points are exported through the .def file (matches the SDK's own
# declaration linkage, which __declspec(dllexport) would conflict with).
set_target_properties(wcl-vcam PROPERTIES
    OUTPUT_NAME "wcl-vcam"
    PREFIX ""
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
    LINK_FLAGS "/DEF:${CMAKE_CURRENT_SOURCE_DIR}/wcl-vcam.def"
)

target_link_libraries(wcl-vcam PRIVATE
    advapi32
    mfplat
    mfuuid
    ole32
    oleaut32
)

# Registration helper: COM (HKLM) + MFCreateVirtualCamera. Needs an elevated shell.
add_executable(wcl-vcam-register tools/register.cpp)
target_include_directories(wcl-vcam-register PRIVATE include)
target_compile_definitions(wcl-vcam-register PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)
set_target_properties(wcl-vcam-register PROPERTIES
    OUTPUT_NAME "wcl-vcam-register"
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
)
target_link_libraries(wcl-vcam-register PRIVATE advapi32 mfplat mfuuid ole32)
