cmake_minimum_required(VERSION 3.20)
project(modloader_rt64 LANGUAGES C CXX)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(rt64_source "${CMAKE_CURRENT_SOURCE_DIR}/../rt64")

set(CMAKE_MSVC_RUNTIME_LIBRARY MultiThreaded) # as the cc crate builds Parallel-RDP
set(RT64_STATIC ON CACHE BOOL "" FORCE)
set(RT64_EXTENDED_RDRAM ON CACHE BOOL "" FORCE)
set(ZSTD_BUILD_PROGRAMS OFF CACHE BOOL "" FORCE)
add_subdirectory("${rt64_source}" rt64)
get_target_property(RT64_SOURCES rt64 SOURCES)
list(FILTER RT64_SOURCES EXCLUDE REGEX "(rt64_application_window|imgui_impl_sdl2_custom)\\.cpp$")
set_target_properties(rt64 PROPERTIES SOURCES "${RT64_SOURCES};${CMAKE_CURRENT_SOURCE_DIR}/application_window_none.cpp")
target_compile_definitions(plume PRIVATE PLUME_NO_VOLK_IMPLEMENTATION) # the adapter links Parallel-RDP's newer volk

# see RT64's headers as RT64's sources do
get_directory_property(RT64_INCLUDES DIRECTORY "${rt64_source}" INCLUDE_DIRECTORIES)
get_directory_property(RT64_DEFINITIONS DIRECTORY "${rt64_source}" COMPILE_DEFINITIONS)

add_library(modloader_rt64 STATIC modloader_rt64.cpp swap_chain_vulkan.cpp)
if (WIN32)
    target_sources(modloader_rt64 PRIVATE swap_chain_d3d12.cpp)
endif ()
set_target_properties(modloader_rt64 PROPERTIES CXX_STANDARD 20 CXX_STANDARD_REQUIRED ON)
target_include_directories(modloader_rt64 PRIVATE ${RT64_INCLUDES} ${CMAKE_BINARY_DIR}/src)
target_compile_definitions(modloader_rt64 PRIVATE ${RT64_DEFINITIONS})
if(CMAKE_CXX_COMPILER_ID MATCHES "Clang")
    target_compile_options(modloader_rt64 PRIVATE -Wno-unused-parameter -Wno-nullability-completeness)
endif()
target_link_libraries(modloader_rt64 PUBLIC rt64)
