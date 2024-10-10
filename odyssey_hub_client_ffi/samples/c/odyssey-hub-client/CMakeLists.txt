# CMake project for odyssey-hub-client, include source and define
# project specific logic here.

cmake_minimum_required (VERSION 3.8)

# Enable Hot Reload for MSVC compilers if supported.
if (POLICY CMP0141)
  cmake_policy(SET CMP0141 NEW)
  set(CMAKE_MSVC_DEBUG_INFORMATION_FORMAT "$<IF:$<AND:$<C_COMPILER_ID:MSVC>,$<CXX_COMPILER_ID:MSVC>>,$<$<CONFIG:Debug,RelWithDebInfo>:EditAndContinue>,$<$<CONFIG:Debug,RelWithDebInfo>:ProgramDatabase>>")
endif()

# Set CMP0111 to NEW to avoid the warning
if (POLICY CMP0111)
  cmake_policy(SET CMP0111 NEW)
endif()

project("odyssey-hub-client")

# Add source to this project's executable.
add_executable(odyssey-hub-client
    "${CMAKE_CURRENT_SOURCE_DIR}/src/odyssey-hub-client.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/odyssey-hub-client.h"
)

if (CMAKE_VERSION VERSION_GREATER 3.12)
  set_property(TARGET odyssey-hub-client PROPERTY CXX_STANDARD 20)
endif()

# Define the platform-specific shared library extension
if (WIN32)
    set(SHARED_LIB_EXT "dll")
    set(IMPORT_LIB_EXT "dll.lib")
else()
    set(SHARED_LIB_EXT "so")
endif()

# Determine the target directory based on the build configuration (Debug or Release)
if (CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../../target/debug")
else()
    set(TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../../../target/release")
endif()

# Full path to the shared library and import library (for Windows)
set(SHARED_LIB_PATH "${TARGET_DIR}/odyssey_hub_client_ffi.${SHARED_LIB_EXT}")
if (WIN32)
    set(IMPORT_LIB_PATH "${TARGET_DIR}/odyssey_hub_client_ffi.${IMPORT_LIB_EXT}")
endif()

# Declare the imported library (shared)
add_library(odyssey-hub-client-lib SHARED IMPORTED)

# Set the properties for the imported library
set_target_properties(odyssey-hub-client-lib PROPERTIES
    IMPORTED_LOCATION "${SHARED_LIB_PATH}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/../../../include"
)

# On Windows, also set the IMPORTED_IMPLIB property for the import library
if (WIN32)
    set_target_properties(odyssey-hub-client-lib PROPERTIES
        IMPORTED_IMPLIB "${IMPORT_LIB_PATH}"
    )
endif()

# Link the executable with the imported shared library
target_link_libraries(odyssey-hub-client PRIVATE odyssey-hub-client-lib)

# Add the include directory for the odyssey-hub-client executable
target_include_directories(odyssey-hub-client PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/../../../include")

# Copy the shared library to the output directory after building the executable
add_custom_command(TARGET odyssey-hub-client POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
    "${SHARED_LIB_PATH}" "$<TARGET_FILE_DIR:odyssey-hub-client>"
)

# TODO: Add tests and install targets if needed.
