# Sample C++ client linked against the ohc shared library from a local cargo build.

cmake_minimum_required (VERSION 3.8)

if (CMAKE_C_COMPILER STREQUAL "cl.exe")
	string(APPEND CMAKE_C_FLAGS " /experimental:c11atomics")
endif()

# MSVC Hot Reload (Edit and Continue) where the policy exists.
if (POLICY CMP0141)
  cmake_policy(SET CMP0141 NEW)
  set(CMAKE_MSVC_DEBUG_INFORMATION_FORMAT "$<IF:$<AND:$<C_COMPILER_ID:MSVC>,$<CXX_COMPILER_ID:MSVC>>,$<$<CONFIG:Debug,RelWithDebInfo>:EditAndContinue>,$<$<CONFIG:Debug,RelWithDebInfo>:ProgramDatabase>>")
endif()

# Avoids the CMP0111 policy warning.
if (POLICY CMP0111)
  cmake_policy(SET CMP0111 NEW)
endif()

project("odyssey-hub-client")

add_executable(odyssey-hub-client
    "${CMAKE_CURRENT_SOURCE_DIR}/src/odyssey-hub-client.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/odyssey-hub-client.hpp"
)

if (CMAKE_VERSION VERSION_GREATER 3.12)
  set_property(TARGET odyssey-hub-client PROPERTY C_STANDARD 11)
endif()

if (WIN32)
    set(SHARED_LIB_EXT "dll")
    set(IMPORT_LIB_EXT "dll.lib")
else()
    set(SHARED_LIB_EXT "so")
endif()

if (CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../../target/debug")
else()
    set(TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../../target/release")
endif()

set(SHARED_LIB_PATH "${TARGET_DIR}/ohc.${SHARED_LIB_EXT}")
if (WIN32)
    set(IMPORT_LIB_PATH "${TARGET_DIR}/ohc.${IMPORT_LIB_EXT}")
endif()

add_library(odyssey-hub-client-lib SHARED IMPORTED)

set_target_properties(odyssey-hub-client-lib PROPERTIES
    IMPORTED_LOCATION "${SHARED_LIB_PATH}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/../../../generated/include"
)

if (WIN32)
    set_target_properties(odyssey-hub-client-lib PROPERTIES
        IMPORTED_IMPLIB "${IMPORT_LIB_PATH}"
    )
endif()

target_link_libraries(odyssey-hub-client PRIVATE odyssey-hub-client-lib)

target_include_directories(odyssey-hub-client PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/../../../generated/include")

# The executable needs the shared library beside it at run time.
add_custom_command(TARGET odyssey-hub-client POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
    "${SHARED_LIB_PATH}" "$<TARGET_FILE_DIR:odyssey-hub-client>"
)
