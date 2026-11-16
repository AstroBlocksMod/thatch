# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Builds the Thatch host on Scratch Everywhere!:
#
#   lib/libscratch-everywhere.{so,dylib}        the patched library (LGPL-3.0), shared, so that
#                                               players can swap in a build of their own
#   lib/libthatch_host_scratch_everywhere.a     the adapter, which owns main()
#   lib/licenses.txt                            the license texts a game ships because of this host
#   lib/thatch-host.txt                         what `thatch build --host` has to link and copy
#
# A game is the object file Thatch compiled, the adapter and libthatch_runtime.a linked together,
# with the shared library and the game's .sb3 next to it. `thatch build --host scratch-everywhere`
# makes one from the files above, and the test at the end does just that.
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

add_subdirectory("${SE_PATCHED}" scratch-everywhere-build EXCLUDE_FROM_ALL)

# A Debug build of the library turns on AddressSanitizer by itself (SE_ASAN in its pc.cmake). An
# instrumented library only loads into an executable that links the sanitizer's runtime, which a
# game does not, so the library is built without it in every configuration.
foreach(property COMPILE_OPTIONS LINK_LIBRARIES)
    get_target_property(values scratch-everywhere ${property})
    if(values)
        list(REMOVE_ITEM values -fsanitize=address -fno-omit-frame-pointer)
        set_target_properties(scratch-everywhere PROPERTIES ${property} "${values}")
    endif()
endforeach()

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

# ---- what `thatch build --host scratch-everywhere` reads -----------------------------------------

# The description of this host: what to link and what to put next to a game.
file(GENERATE OUTPUT "${HOST_LIB_DIR}/thatch-host.txt" CONTENT
"# Written by the build of hosts/scratch-everywhere; read by `thatch build --host`.
adapter = $<TARGET_FILE_NAME:thatch-host-scratch-everywhere>
library = $<TARGET_FILE_NAME:scratch-everywhere>
notices = licenses.txt
linker = c++
project = sb3
")

# The license texts a game has to ship because of this host: the LGPL and the GPL, where the
# library's source is, and the notices of the libraries built into it. Those are found through
# the directories the library's build uses: every directory CMake entered for it, and every
# include directory it compiles with.
function(thatch_collect_directories dir out)
    set(found "${dir}")
    get_property(children DIRECTORY "${dir}" PROPERTY SUBDIRECTORIES)
    foreach(child IN LISTS children)
        thatch_collect_directories("${child}" more)
        list(APPEND found ${more})
    endforeach()
    set(${out} "${found}" PARENT_SCOPE)
endfunction()
thatch_collect_directories("${SE_PATCHED}" SE_DIRECTORIES)
list(JOIN SE_DIRECTORIES "\n" SE_DIRECTORY_LINES)
set(SE_SOURCE_DIRS_FILE "${CMAKE_CURRENT_BINARY_DIR}/scratch-everywhere-dirs.txt")
file(GENERATE OUTPUT "${SE_SOURCE_DIRS_FILE}" CONTENT
    "${SE_DIRECTORY_LINES}\n$<JOIN:$<TARGET_PROPERTY:scratch-everywhere,INCLUDE_DIRECTORIES>,\n>\n")
add_custom_command(
    OUTPUT "${HOST_LIB_DIR}/licenses.txt"
    COMMAND "${CMAKE_COMMAND}"
        "-DOUT=${HOST_LIB_DIR}/licenses.txt"
        "-DSE_COMMIT=${SE_COMMIT}"
        "-DSE_LICENSE=${SE_PATCHED}/LICENSE"
        "-DGPL_TEXT=${CMAKE_CURRENT_SOURCE_DIR}/LICENSES/GPL-3.0.txt"
        "-DPATCH=${SE_PATCH}"
        "-DSOURCE_DIRS=${SE_SOURCE_DIRS_FILE}"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/notices.cmake"
    DEPENDS notices.cmake "${SE_PATCH}" LICENSES/GPL-3.0.txt "${SE_SOURCE_DIRS_FILE}"
    VERBATIM)

add_custom_target(thatch-host ALL DEPENDS
    scratch-everywhere thatch-host-scratch-everywhere "${HOST_LIB_DIR}/licenses.txt")

# ---- the Cargo workspace's build output ----------------------------------------------------------

# The compiler and the runtime come from the Cargo workspace, which is built first. The test uses
# both, and the install step installs the runtime library with the host.
set(THATCH_TARGET_DIR "" CACHE PATH "The directory with thatch and libthatch_runtime.a (target/debug or target/release)")
if(NOT THATCH_TARGET_DIR)
    foreach(candidate "${THATCH_ROOT}/target/release" "${THATCH_ROOT}/target/debug")
        if(EXISTS "${candidate}/thatch" AND EXISTS "${candidate}/libthatch_runtime.a")
            set(THATCH_TARGET_DIR "${candidate}")
        endif()
    endforeach()
endif()

# ---- installing: where an installed thatch finds the host ---------------------------------------

# `cmake --install` puts the host into Thatch's data directory, where any thatch looks for it,
# including one from `cargo install`, which comes without a host and without the runtime library.
# The directory is worked out the way `thatch` works it out.
if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
    if(DEFINED ENV{THATCH_HOME} AND NOT "$ENV{THATCH_HOME}" STREQUAL "")
        set(THATCH_DATA_DIR "$ENV{THATCH_HOME}")
    elseif(APPLE)
        set(THATCH_DATA_DIR "$ENV{HOME}/Library/Application Support/thatch")
    elseif(DEFINED ENV{XDG_DATA_HOME} AND NOT "$ENV{XDG_DATA_HOME}" STREQUAL "")
        set(THATCH_DATA_DIR "$ENV{XDG_DATA_HOME}/thatch")
    else()
        set(THATCH_DATA_DIR "$ENV{HOME}/.local/share/thatch")
    endif()
    set(CMAKE_INSTALL_PREFIX "${THATCH_DATA_DIR}" CACHE PATH "Thatch's data directory" FORCE)
endif()

# The host goes to hosts/scratch-everywhere as the build leaves it in lib/: the paths in its
# description are relative, so it works from anywhere.
install(FILES
    "$<TARGET_FILE:scratch-everywhere>"
    "$<TARGET_FILE:thatch-host-scratch-everywhere>"
    "${HOST_LIB_DIR}/licenses.txt"
    "${HOST_LIB_DIR}/thatch-host.txt"
    DESTINATION hosts/scratch-everywhere)
# The runtime library the host was built and tested with, with its notices, for a thatch that has
# none next to it. A program built against another version of the ABI is refused when it loads.
if(EXISTS "${THATCH_TARGET_DIR}/libthatch_runtime.a")
    install(FILES
        "${THATCH_TARGET_DIR}/libthatch_runtime.a"
        "${THATCH_TARGET_DIR}/libthatch_runtime.licenses.txt"
        DESTINATION lib)
else()
    message(STATUS "libthatch_runtime.a was not found (build the Cargo workspace, or set THATCH_TARGET_DIR); the install step will not install it")
endif()

# ---- the test: a game built the way `thatch build` builds one -----------------------------------

include(CTest)
if(BUILD_TESTING)
    if(NOT EXISTS "${THATCH_TARGET_DIR}/thatch" OR NOT EXISTS "${THATCH_TARGET_DIR}/libthatch_runtime.a")
        message(STATUS "thatch was not found (build the Cargo workspace, or set THATCH_TARGET_DIR); skipping the host tests")
    else()
        message(STATUS "Testing with ${THATCH_TARGET_DIR}/thatch")
        set(TEST_PROJECT "${CMAKE_CURRENT_SOURCE_DIR}/tests/project")
        file(GLOB TEST_PROJECT_FILES CONFIGURE_DEPENDS "${TEST_PROJECT}/project.json" "${TEST_PROJECT}/*.svg" "${TEST_PROJECT}/*.png")
        set(TEST_DIR "${CMAKE_CURRENT_BINARY_DIR}/test")
        file(MAKE_DIRECTORY "${TEST_DIR}")

        # The project as an .sb3, and the game `thatch build --host` makes of it: the executable
        # with the shared library, the project file and the license notices next to it.
        list(TRANSFORM TEST_PROJECT_FILES REPLACE "^.*/" "" OUTPUT_VARIABLE TEST_PROJECT_NAMES)
        add_custom_command(
            OUTPUT "${TEST_DIR}/project.sb3"
            COMMAND "${CMAKE_COMMAND}" -E tar cf "${TEST_DIR}/project.sb3" --format=zip ${TEST_PROJECT_NAMES}
            WORKING_DIRECTORY "${TEST_PROJECT}"
            DEPENDS ${TEST_PROJECT_FILES}
            VERBATIM)
        add_custom_command(
            OUTPUT "${TEST_DIR}/game" "${TEST_DIR}/game.sb3" "${TEST_DIR}/game.licenses.txt"
            COMMAND "${THATCH_TARGET_DIR}/thatch" build "${TEST_DIR}/project.sb3"
                --host scratch-everywhere --host-dir "${HOST_LIB_DIR}" -o "${TEST_DIR}/game"
            DEPENDS "${TEST_DIR}/project.sb3" "${THATCH_TARGET_DIR}/thatch" "${THATCH_TARGET_DIR}/libthatch_runtime.a"
                thatch-host "${HOST_LIB_DIR}/licenses.txt"
            VERBATIM)
        add_custom_target(thatch-host-tests ALL DEPENDS "${TEST_DIR}/game")

        add_test(
            NAME sprites_follow_the_runtime
            COMMAND "${CMAKE_COMMAND}"
                "-DGAME=${TEST_DIR}/game"
                "-DNOTICES=${TEST_DIR}/game.licenses.txt"
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
