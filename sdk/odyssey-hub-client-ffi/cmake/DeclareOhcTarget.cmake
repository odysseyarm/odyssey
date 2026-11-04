# Declares the `ohc` IMPORTED target against a prebuilt lib in OHC_LIB_DIR
# and headers in OHC_INCLUDE_DIR (both must be set by the includer).
#
# Included by the top-level CMakeLists.txt (before install: FetchContent or
# add_subdirectory) and by the installed odyssey_hub_clientConfig.cmake (after
# install, through find_package()). An IMPORTED target cannot be
# install(EXPORT)-ed like a normal library, so each context re-declares it
# against its own paths.

if(TARGET ohc)
    return()
endif()

if(NOT DEFINED OHC_LIB_DIR OR NOT DEFINED OHC_INCLUDE_DIR)
    message(FATAL_ERROR "OHC_LIB_DIR and OHC_INCLUDE_DIR must be set before including DeclareOhcTarget.cmake")
endif()

# Directory of the .dll itself; its .dll.lib import library is in OHC_LIB_DIR.
# The two differ only after install, where install() puts runtime DLLs in bin/
# and import libs in lib/. Before install everything sits in the bundle's one
# lib/ dir, so default to OHC_LIB_DIR and let Config.cmake.in override it.
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
elseif(UNIX) # if(LINUX) needs CMake 3.25
    if("${CMAKE_SYSTEM_PROCESSOR}" MATCHES "^(arm64|aarch64)$")
        set(OHC_OS_ARCH "linux_arm64")
    else()
        set(OHC_OS_ARCH "linux_x64")
    endif()
elseif(WIN32)
    # TODO: no win_arm64 library is built, so arm64 Windows gets the x64 one.
    set(OHC_OS_ARCH "win_x64")
else()
    message(FATAL_ERROR "Unsupported platform ${CMAKE_SYSTEM_NAME}/${CMAKE_SYSTEM_PROCESSOR}, can't find a prebuilt ohc library.")
endif()

# The __<os>_<arch> suffix stays on the installed file, as rerun-io/rerun's
# rerun_c does. The installed Config.cmake.in re-includes this module, which
# recomputes OHC_OS_ARCH and so resolves the same filename. Non-CMake consumers,
# such as a plain vcxproj under vcpkg's classic MSBuild integration, reference
# this exact filename by hand: only find_package() and imported-target
# resolution pick the right file automatically.
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

    # A raw Rust staticlib needs its runtime's OS dependencies linked by
    # whoever embeds it; the shared library already bundles them. This list
    # comes from the dependency stack (tokio, tonic/TLS, nusb and interprocess
    # for USB/IPC) and has not been checked against a real static link on
    # every platform, so the first CI static link may need it adjusted.
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
