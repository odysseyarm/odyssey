# ------------------------------------------------------------------------------
# Odyssey Hub Client C/C++ SDK
#
# Ships prebuilt binaries only (no source build) — native libs are
# cross-compiled in odysseyarm/odyssey-desktop's CI. This declares an
# IMPORTED `ohc` target (see cmake/DeclareOhcTarget.cmake) pointing at the
# right prebuilt file for the current platform/arch/linkage, and installs
# headers + a CMake package config so `find_package(odyssey_hub_client
# CONFIG)` works.
#
# Works three ways with no changes needed:
#   - As a vcpkg port (odysseyarm/vcpkg-registry, port "odyssey-hub-client")
#   - Via CMake FetchContent, pointed at a release zip
#   - Vendored directly / add_subdirectory()'d
# ------------------------------------------------------------------------------

cmake_minimum_required(VERSION 3.16)

project(odyssey_hub_client LANGUAGES C CXX)

option(OHC_INSTALL_C_HEADER "Install the C header (ohc.h)" ON)
option(OHC_INSTALL_CXX_HEADER "Install the C++ headers (ohc.hpp, ohc_extras.hpp)" ON)

set(OHC_LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/lib")
set(OHC_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/include")
include("${CMAKE_CURRENT_SOURCE_DIR}/cmake/DeclareOhcTarget.cmake")

# -----------------------------------------------------------------------------
# Installation.
set(OHC_INSTALL_CMAKE_DIR "lib/cmake/odyssey_hub_client")

if(OHC_INSTALL_C_HEADER)
    install(FILES "${OHC_INCLUDE_DIR}/ohc.h" DESTINATION include)
endif()

if(OHC_INSTALL_CXX_HEADER)
    install(FILES
        "${OHC_INCLUDE_DIR}/ohc.hpp"
        "${OHC_INCLUDE_DIR}/ohc_extras.hpp"
        DESTINATION include
    )
endif()

# CMake can't install() an IMPORTED target's binary directly, so it's added
# as a plain file instead.
if(OHC_LINK_SHARED AND WIN32)
    install(FILES "${OHC_LIB_DIR}/ohc__${OHC_OS_ARCH}.dll" DESTINATION bin)
    install(FILES "${OHC_LIB_DIR}/ohc__${OHC_OS_ARCH}.dll.lib" DESTINATION lib)
else()
    get_target_property(OHC_LIB_LOCATION ohc IMPORTED_LOCATION)
    install(FILES "${OHC_LIB_LOCATION}" DESTINATION lib)
endif()

# Install the module alongside the config so it can re-declare `ohc` fresh
# after find_package() (see the module's own header comment for why).
install(FILES "${CMAKE_CURRENT_SOURCE_DIR}/cmake/DeclareOhcTarget.cmake" DESTINATION ${OHC_INSTALL_CMAKE_DIR})

include(CMakePackageConfigHelpers)

# No ${VERSION} equivalent is available when this is configured standalone
# (FetchContent / add_subdirectory) — that variable only exists inside a
# vcpkg portfile's own scope. Read it from a plain VERSION file instead,
# which CI writes alongside everything else during release packaging.
file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/VERSION" OHC_SDK_VERSION LIMIT_COUNT 1)

configure_package_config_file(${CMAKE_CURRENT_SOURCE_DIR}/Config.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/odyssey_hub_clientConfig.cmake"
    INSTALL_DESTINATION ${OHC_INSTALL_CMAKE_DIR}
    NO_SET_AND_CHECK_MACRO
    NO_CHECK_REQUIRED_COMPONENTS_MACRO
)

write_basic_package_version_file(
    ${CMAKE_CURRENT_BINARY_DIR}/odyssey_hub_clientConfigVersion.cmake
    VERSION ${OHC_SDK_VERSION}
    COMPATIBILITY SameMajorVersion
)

install(FILES
    "${CMAKE_CURRENT_BINARY_DIR}/odyssey_hub_clientConfig.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/odyssey_hub_clientConfigVersion.cmake"
    DESTINATION ${OHC_INSTALL_CMAKE_DIR}
)
