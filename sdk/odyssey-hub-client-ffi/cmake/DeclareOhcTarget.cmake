# Declares the `ohc` IMPORTED target against a prebuilt lib in OHC_LIB_DIR
# and headers in OHC_INCLUDE_DIR (both must be set by the includer).
#
# Included both by the top-level CMakeLists.txt (pre-install / FetchContent /
# add_subdirectory use) and by the installed odyssey_hub_clientConfig.cmake
# (post-install, via find_package()). An IMPORTED target can't be
# install(EXPORT)-ed like a normal library, so each context re-declares it
# fresh against its own paths instead of sharing one CMake target object.

if(TARGET ohc)
    return()
endif()

if(NOT DEFINED OHC_LIB_DIR OR NOT DEFINED OHC_INCLUDE_DIR)
    message(FATAL_ERROR "OHC_LIB_DIR and OHC_INCLUDE_DIR must be set before including DeclareOhcTarget.cmake")
endif()

# Where the .dll itself lives (as opposed to its .dll.lib import library, in
# OHC_LIB_DIR). These only differ post-install: install() puts runtime DLLs
# in bin/ and import libs in lib/, but pre-install everything sits flat
# together in the bundle's one lib/ dir — so default to OHC_LIB_DIR and let
# Config.cmake.in (the only context where they diverge) override it.
if(NOT DEFINED OHC_BIN_DIR)
    set(OHC_BIN_DIR "${OHC_LIB_DIR}")
endif()

# Under vcpkg, VCPKG_LIBRARY_LINKAGE ("static"/"dynamic") picks the linkage.
# Standalone (FetchContent / add_subdirectory), BUILD_SHARED_LIBS does.
if(DEFINED VCPKG_LIBRARY_LINKAGE)
    if(VCPKG_LIBRARY_LINKAGE STREQUAL "dynamic")
        set(OHC_LINK_SHARED_DEFAULT ON)
    else()
        set(OHC_LINK_SHARED_DEFAULT OFF)
    endif()
else()
    set(OHC_LINK_SHARED_DEFAULT ${BUILD_SHARED_LIBS})
endif()

option(OHC_LINK_SHARED "Link the shared/dynamic ohc library instead of the static one" ${OHC_LINK_SHARED_DEFAULT})

if(APPLE)
    if("${CMAKE_SYSTEM_PROCESSOR}" MATCHES "^(arm64|aarch64)$")
        set(OHC_OS_ARCH "macos_arm64")
    else()
        set(OHC_OS_ARCH "macos_x64")
    endif()
elseif(UNIX) # if(LINUX) # CMake 3.25
    if("${CMAKE_SYSTEM_PROCESSOR}" MATCHES "^(arm64|aarch64)$")
        set(OHC_OS_ARCH "linux_arm64")
    else()
        set(OHC_OS_ARCH "linux_x64")
    endif()
elseif(WIN32)
    # TODO: arm64 Windows support.
    set(OHC_OS_ARCH "win_x64")
else()
    message(FATAL_ERROR "Unsupported platform ${CMAKE_SYSTEM_NAME}/${CMAKE_SYSTEM_PROCESSOR}, can't find a prebuilt ohc library.")
endif()

# The __<os>_<arch> suffix stays on the installed file too (matching
# rerun-io/rerun's rerun_c: they don't rename on install either — the
# Config.cmake.in just bakes in the resolved filename from the original
# build, same idea as OHC_OS_ARCH being recomputed here). Non-CMake
# consumers (e.g. a plain vcxproj under vcpkg's classic MSBuild integration)
# have to reference this exact filename by hand either way — CMake's
# find_package()/imported-target resolution is the only thing that can pick
# the right file automatically, and only CMake consumers get that for free.
set(OHC_LIB_SUFFIX "__${OHC_OS_ARCH}")

if(OHC_LINK_SHARED)
    add_library(ohc SHARED IMPORTED GLOBAL)
    if(WIN32)
        set_target_properties(ohc PROPERTIES
            IMPORTED_LOCATION "${OHC_BIN_DIR}/ohc${OHC_LIB_SUFFIX}.dll"
            IMPORTED_IMPLIB "${OHC_LIB_DIR}/ohc${OHC_LIB_SUFFIX}.dll.lib"
        )
    elseif(APPLE)
        set_target_properties(ohc PROPERTIES IMPORTED_LOCATION "${OHC_LIB_DIR}/libohc${OHC_LIB_SUFFIX}.dylib")
    else()
        set_target_properties(ohc PROPERTIES IMPORTED_LOCATION "${OHC_LIB_DIR}/libohc${OHC_LIB_SUFFIX}.so")
    endif()
else()
    add_library(ohc STATIC IMPORTED GLOBAL)
    if(WIN32)
        set_target_properties(ohc PROPERTIES IMPORTED_LOCATION "${OHC_LIB_DIR}/ohc${OHC_LIB_SUFFIX}.lib")
    else()
        set_target_properties(ohc PROPERTIES IMPORTED_LOCATION "${OHC_LIB_DIR}/libohc${OHC_LIB_SUFFIX}.a")
    endif()

    # A raw Rust staticlib needs its runtime's OS dependencies linked in by
    # whoever embeds it (dynamic linkage already bundles these). This list is
    # best-effort from our actual dependency stack (tokio, tonic/TLS, nusb +
    # interprocess for USB/IPC) — not verified against a real static link on
    # every platform yet, may need adjusting once CI actually tries it.
    if(WIN32)
        target_link_libraries(ohc INTERFACE
            ws2_32
            userenv
            ntdll
            bcrypt
            advapi32
            setupapi
            cfgmgr32
            secur32
            crypt32
            ncrypt
            iphlpapi
        )
    elseif(APPLE)
        target_link_libraries(ohc INTERFACE
            "-framework CoreFoundation"
            "-framework IOKit"
            "-framework Security"
            pthread
        )
    else()
        find_package(PkgConfig REQUIRED)
        pkg_check_modules(OHC_UDEV REQUIRED IMPORTED_TARGET libudev)
        pkg_check_modules(OHC_DBUS REQUIRED IMPORTED_TARGET dbus-1)
        target_link_libraries(ohc INTERFACE PkgConfig::OHC_UDEV PkgConfig::OHC_DBUS m dl pthread)
    endif()
endif()

target_include_directories(ohc INTERFACE "${OHC_INCLUDE_DIR}")
