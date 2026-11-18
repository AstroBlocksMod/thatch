# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Builds the Thatch host on Scratch Everywhere!:
#
#   lib/libscratch-everywhere.{so,dylib}        the patched library (LGPL-3.0-or-later), shared, so
#                                               that players can swap in a build of their own
#   lib/libthatch_host_scratch_everywhere.a     the adapter, which owns main()
#
# A game is the object file Thatch compiled, the adapter and libthatch_runtime.a linked together,
# with the shared library and the game's .sb3 next to it. The test at the end builds one.
#
# The library is built from the ScratchEverywhere submodule with patches/host-hooks.patch applied
# to a copy in the build directory; the submodule itself is never touched.

cmake_minimum_required(VERSION 3.21)
project(thatch_host_scratch_everywhere LANGUAGES C CXX)

get_filename_component(THATCH_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
set(SE_SUBMODULE "${CMAKE_CURRENT_SOURCE_DIR}/ScratchEverywhere")
set(SE_PATCH "${CMAKE_CURRENT_SOURCE_DIR}/patches/host-hooks.patch")
set(SE_PATCHED "${CMAKE_CURRENT_BINARY_DIR}/scratch-everywhere-src")

# ---- a patched copy of Scratch Everywhere! ---------------------------------------------------

find_package(Git REQUIRED)
if(NOT EXISTS "${SE_SUBMODULE}/CMakeLists.txt")
    message(FATAL_ERROR
        "The Scratch Everywhere! submodule is missing. Fetch it with:\n"
        "  git submodule update --init hosts/scratch-everywhere/ScratchEverywhere")
endif()

execute_process(
    COMMAND "${GIT_EXECUTABLE}" -C "${SE_SUBMODULE}" rev-parse HEAD
    OUTPUT_VARIABLE SE_COMMIT OUTPUT_STRIP_TRAILING_WHITESPACE
    RESULT_VARIABLE SE_COMMIT_RESULT)
if(NOT SE_COMMIT_RESULT EQUAL 0)
    message(FATAL_ERROR "Could not read the commit of the Scratch Everywhere! submodule.")
endif()
file(SHA256 "${SE_PATCH}" SE_PATCH_HASH)
set(SE_STAMP "${SE_COMMIT} ${SE_PATCH_HASH}")
set(SE_STAMP_FILE "${CMAKE_CURRENT_BINARY_DIR}/scratch-everywhere-src.stamp")
set(SE_OLD_STAMP "")
if(EXISTS "${SE_STAMP_FILE}")
    file(READ "${SE_STAMP_FILE}" SE_OLD_STAMP)
endif()
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${SE_PATCH}")

if(NOT SE_OLD_STAMP STREQUAL SE_STAMP OR NOT EXISTS "${SE_PATCHED}/CMakeLists.txt")
    message(STATUS "Copying Scratch Everywhere! ${SE_COMMIT} and applying host-hooks.patch")
    file(REMOVE_RECURSE "${SE_PATCHED}")
    file(MAKE_DIRECTORY "${SE_PATCHED}")
    # The files the submodule tracks, without anything left over in its working directory.
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${SE_SUBMODULE}" checkout-index --all --force "--prefix=${SE_PATCHED}/"
        RESULT_VARIABLE SE_COPY_RESULT)
    if(NOT SE_COPY_RESULT EQUAL 0)
        message(FATAL_ERROR "Could not copy the Scratch Everywhere! submodule to ${SE_PATCHED}.")
    endif()
    # The copy may lie inside the Thatch checkout. The ceiling keeps git from treating it as part
    # of that repository, so the patch's paths are taken from the copy's own root.
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "GIT_CEILING_DIRECTORIES=${CMAKE_CURRENT_BINARY_DIR}"
                "${GIT_EXECUTABLE}" apply --whitespace=nowarn "${SE_PATCH}"
        WORKING_DIRECTORY "${SE_PATCHED}"
        RESULT_VARIABLE SE_PATCH_RESULT)
    if(NOT SE_PATCH_RESULT EQUAL 0 OR NOT EXISTS "${SE_PATCHED}/include/runtime/host_hooks.hpp")
        file(REMOVE "${SE_STAMP_FILE}")
        message(FATAL_ERROR
            "patches/host-hooks.patch does not apply to Scratch Everywhere! ${SE_COMMIT}. "
            "The patch has to be updated together with the submodule.")
    endif()
    file(WRITE "${SE_STAMP_FILE}" "${SE_STAMP}")
endif()

# ---- the library ---------------------------------------------------------------------------------

# A shared library: the LGPL asks that whoever gets a game can replace the library in it.
set(SE_LIBRARY ON CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS ON CACHE BOOL "" FORCE)
set(SE_OUTPUT_NAME "scratch-everywhere" CACHE STRING "" FORCE)
# DECtalk's license is unclear, so it stays out. The JIT of later versions starts background
# threads, which the Thatch runtime does not expect around it, so it stays out as well.
set(SE_DECTALK OFF CACHE BOOL "" FORCE)
set(SE_JIT OFF CACHE BOOL "" FORCE)
# What a game compiled by Thatch has no use for: the library's own menu comes off with
# SE_LIBRARY; these are the parts that reach for the network or load code at run time.
set(SE_CLOUDVARS OFF CACHE BOOL "" FORCE)
set(SE_DOWNLOAD OFF CACHE BOOL "" FORCE)
set(SE_CUSTOM_EXTENSIONS OFF CACHE BOOL "" FORCE)
set(SE_INSPECTOR OFF CACHE BOOL "" FORCE)
# SDL2 unless the build picks another backend (-DSE_RENDERER=headless and so on).
set(SE_RENDERER "sdl2" CACHE STRING "The renderer Scratch Everywhere! draws with")
set(SE_WINDOWING "sdl2" CACHE STRING "The windowing backend of Scratch Everywhere!")
set(SE_AUDIO_ENGINE "sdl2" CACHE STRING "The audio engine of Scratch Everywhere!")

# The library fetches its dependencies with Catalog, which otherwise stops to ask whether it may
# install packages on the system. It builds what the system lacks from source instead.
set(ENV{CATALOG_NON_INTERACTIVE} 1)

# build/compile_commands.json lists the include directories of every library compiled into
# Scratch Everywhere!, which is how the docs find their license notices. Makefile and Ninja
# generators write it.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

add_subdirectory("${SE_PATCHED}" scratch-everywhere-build EXCLUDE_FROM_ALL)

set(HOST_LIB_DIR "${CMAKE_CURRENT_BINARY_DIR}/lib")
set_target_properties(scratch-everywhere PROPERTIES
    LIBRARY_OUTPUT_DIRECTORY "${HOST_LIB_DIR}"
    RUNTIME_OUTPUT_DIRECTORY "${HOST_LIB_DIR}"
    ARCHIVE_OUTPUT_DIRECTORY "${HOST_LIB_DIR}")

# ---- the adapter ---------------------------------------------------------------------------------

add_library(thatch-host-scratch-everywhere STATIC adapter/thatch_host.cpp)
set_target_properties(thatch-host-scratch-everywhere PROPERTIES
    OUTPUT_NAME thatch_host_scratch_everywhere
    ARCHIVE_OUTPUT_DIRECTORY "${HOST_LIB_DIR}"
    CXX_STANDARD 17
    CXX_STANDARD_REQUIRED ON
    POSITION_INDEPENDENT_CODE ON)
# The adapter includes the library's headers, whose classes change shape with the library's build
# options, so it is compiled with exactly the library's include directories and definitions. It is
# not linked against the library's own dependencies: those are inside the shared library.
target_include_directories(thatch-host-scratch-everywhere PRIVATE
    "${THATCH_ROOT}/crates/thatch-abi/include"
    "$<TARGET_PROPERTY:scratch-everywhere,INCLUDE_DIRECTORIES>")
target_compile_definitions(thatch-host-scratch-everywhere PRIVATE
    "$<FILTER:$<TARGET_PROPERTY:scratch-everywhere,COMPILE_DEFINITIONS>,EXCLUDE,^SE_BUILD_LIBRARY$>")
if(WIN32)
    target_compile_definitions(thatch-host-scratch-everywhere PRIVATE SE_DLL)
endif()
add_dependencies(thatch-host-scratch-everywhere scratch-everywhere)

add_custom_target(thatch-host ALL DEPENDS scratch-everywhere thatch-host-scratch-everywhere)

# ---- the test: a game built the way `thatch build` builds one -----------------------------------

include(CTest)
if(BUILD_TESTING)
    # The compiler and the runtime come from the Cargo workspace, which is built first.
    set(THATCH_TARGET_DIR "" CACHE PATH "The directory with thatch and libthatch_runtime.a (target/debug or target/release)")
    if(NOT THATCH_TARGET_DIR)
        foreach(candidate "${THATCH_ROOT}/target/release" "${THATCH_ROOT}/target/debug")
            if(EXISTS "${candidate}/thatch" AND EXISTS "${candidate}/libthatch_runtime.a")
                set(THATCH_TARGET_DIR "${candidate}")
            endif()
        endforeach()
    endif()
    if(NOT EXISTS "${THATCH_TARGET_DIR}/thatch" OR NOT EXISTS "${THATCH_TARGET_DIR}/libthatch_runtime.a")
        message(STATUS "thatch was not found (build the Cargo workspace, or set THATCH_TARGET_DIR); skipping the host tests")
    else()
        message(STATUS "Testing with ${THATCH_TARGET_DIR}/thatch")
        set(TEST_PROJECT "${CMAKE_CURRENT_SOURCE_DIR}/tests/project")
        file(GLOB TEST_PROJECT_FILES CONFIGURE_DEPENDS "${TEST_PROJECT}/project.json" "${TEST_PROJECT}/*.svg" "${TEST_PROJECT}/*.png")
        set(TEST_DIR "${CMAKE_CURRENT_BINARY_DIR}/test")
        file(MAKE_DIRECTORY "${TEST_DIR}")

        # The project as an .sb3, which the library reads the costumes from, and as an object
        # file without main, which is the game's code.
        list(TRANSFORM TEST_PROJECT_FILES REPLACE "^.*/" "" OUTPUT_VARIABLE TEST_PROJECT_NAMES)
        add_custom_command(
            OUTPUT "${TEST_DIR}/game.sb3"
            COMMAND "${CMAKE_COMMAND}" -E tar cf "${TEST_DIR}/game.sb3" --format=zip ${TEST_PROJECT_NAMES}
            WORKING_DIRECTORY "${TEST_PROJECT}"
            DEPENDS ${TEST_PROJECT_FILES}
            VERBATIM)
        add_custom_command(
            OUTPUT "${TEST_DIR}/game.o"
            COMMAND "${THATCH_TARGET_DIR}/thatch" build "${TEST_DIR}/game.sb3" --object --no-main -o "${TEST_DIR}/game.o"
            DEPENDS "${TEST_DIR}/game.sb3" "${THATCH_TARGET_DIR}/thatch"
            VERBATIM)

        add_executable(thatch-host-test-game "${TEST_DIR}/game.o")
        set_target_properties(thatch-host-test-game PROPERTIES
            LINKER_LANGUAGE CXX
            OUTPUT_NAME game
            RUNTIME_OUTPUT_DIRECTORY "${TEST_DIR}"
            BUILD_RPATH "${HOST_LIB_DIR}")
        set_source_files_properties("${TEST_DIR}/game.o" PROPERTIES EXTERNAL_OBJECT TRUE GENERATED TRUE)
        # What a Rust static library needs from the system, as `thatch build` links it.
        if(APPLE)
            set(THATCH_SYSTEM_LIBS iconv)
        else()
            set(THATCH_SYSTEM_LIBS gcc_s util rt pthread m dl c)
        endif()
        target_link_libraries(thatch-host-test-game PRIVATE
            thatch-host-scratch-everywhere
            scratch-everywhere
            "${THATCH_TARGET_DIR}/libthatch_runtime.a"
            ${THATCH_SYSTEM_LIBS})
        add_custom_target(thatch-host-tests ALL DEPENDS thatch-host-test-game)

        add_test(
            NAME sprites_follow_the_runtime
            COMMAND "${CMAKE_COMMAND}"
                "-DGAME=$<TARGET_FILE:thatch-host-test-game>"
                "-DPROJECT=${TEST_DIR}/game.sb3"
                "-DINPUT=${CMAKE_CURRENT_SOURCE_DIR}/tests/input.txt"
                "-DEXPECTED=${CMAKE_CURRENT_SOURCE_DIR}/tests/expected.txt"
                -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/run.cmake")
        # No window and no sound: SDL draws into memory, so the test runs anywhere, and does the
        # same on a desk as in CI. The library asks SDL for an accelerated renderer, which a
        # machine without a display does not have, so SDL is told to use its software renderer.
        # The other backends ignore these.
        set_tests_properties(sprites_follow_the_runtime PROPERTIES
            ENVIRONMENT "SDL_VIDEODRIVER=dummy;SDL_AUDIODRIVER=dummy;SDL_RENDER_DRIVER=software")
    endif()
endif()
