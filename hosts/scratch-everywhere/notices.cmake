# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Writes the license texts a game built with this host has to ship on top of the notices of the
# Thatch runtime: `thatch build --host` appends this file to the game's OUTPUT.licenses.txt.
#
# Run by the build with:
#   -DOUT=            the file to write
#   -DSE_COMMIT=      the commit of Scratch Everywhere! the library was built from
#   -DSE_LICENSE=     its license text (the LGPL)
#   -DGPL_TEXT=       the text of the GPL, which the LGPL builds on
#   -DPATCH=          the patch that was applied
#   -DSOURCE_DIRS=    a file listing the source and include directories of the library's build,
#                     one per line, from which the libraries built into it are worked out

set(rule "--------------------------------------------------------------------------------")

file(READ "${SE_LICENSE}" lgpl)
file(READ "${GPL_TEXT}" gpl)
file(READ "${PATCH}" patch)

set(text "\
${rule}
Scratch Everywhere!

This program uses Scratch Everywhere! (https://github.com/ScratchEverywhere/ScratchEverywhere), a
library covered by the GNU Lesser General Public License, version 3. The library is the file
named libscratch-everywhere that comes with this program. You may replace it with a version of
your own, and you may change this program and take it apart as far as you need to in order to
make it work with a changed library.

The library was built from commit ${SE_COMMIT} of that repository, with the change shown under
\"The change to Scratch Everywhere!\" below, by the build files in hosts/scratch-everywhere/ of
Thatch (https://github.com/AstroBlocksMod/thatch). Together these are the complete source code of
the library as it is here.

GNU Lesser General Public License, version 3:

${lgpl}
GNU General Public License, version 3, which the license above builds on:

${gpl}
The change to Scratch Everywhere!:

${patch}
")

# ---- the libraries built into the shared library ----------------------------------------------
#
# Scratch Everywhere! fetches the libraries it is built from with Catalog, which unpacks each one
# into a directory named src_<hash> in its cache. Those directories show up among the build's
# source and include directories, and their license files are what gets copied. A library that
# comes from the system instead (SDL2, usually) is not inside the shared library and not listed.

file(STRINGS "${SOURCE_DIRS}" dirs)
set(roots "")
foreach(dir IN LISTS dirs)
    if(dir MATCHES "^(.*/src_[0-9a-f]+)(/.*)?$")
        list(APPEND roots "${CMAKE_MATCH_1}")
    endif()
endforeach()
list(REMOVE_DUPLICATES roots)

set(sections "")
set(names "")
foreach(root IN LISTS roots)
    # An archive unpacks one level down, a clone at the top.
    set(tops "${root}")
    if(IS_DIRECTORY "${root}/src")
        list(APPEND tops "${root}/src")
    endif()

    # What the library is called: where it was cloned from, or what its build calls it.
    set(name "")
    if(EXISTS "${root}/.git")
        execute_process(
            COMMAND git -C "${root}" remote get-url origin
            OUTPUT_VARIABLE name OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
    endif()
    foreach(top IN LISTS tops)
        if(name STREQUAL "" AND EXISTS "${top}/CMakeLists.txt")
            file(READ "${top}/CMakeLists.txt" build_file)
            if(build_file MATCHES "(^|\n)[ \t]*project[ \t\r\n]*\\([ \t\r\n]*\"?([A-Za-z0-9_.+-]+)")
                set(name "${CMAKE_MATCH_2}")
            endif()
        endif()
        # Or the title of its README.
        if(name STREQUAL "" AND EXISTS "${top}/README.md")
            file(STRINGS "${top}/README.md" titles REGEX "^# +[A-Za-z0-9]" LIMIT_COUNT 1)
            if(titles)
                # Without the badges some titles carry.
                string(REGEX REPLACE "^# +" "" name "${titles}")
                string(REGEX REPLACE " *[[(!].*$" "" name "${name}")
                string(STRIP "${name}" name)
            endif()
        endif()
    endforeach()
    if(name STREQUAL "")
        get_filename_component(name "${root}" NAME)
    endif()

    set(files "")
    foreach(top IN LISTS tops)
        file(GLOB found LIST_DIRECTORIES false
            "${top}/LICENSE*" "${top}/LICENCE*" "${top}/License*" "${top}/license*"
            "${top}/COPYING*" "${top}/Copying*" "${top}/copying*"
            "${top}/NOTICE*" "${top}/UNLICENSE*")
        list(APPEND files ${found})
    endforeach()
    list(REMOVE_DUPLICATES files)
    list(SORT files)

    string(APPEND sections "\n${rule}\n${name}\n")
    list(APPEND names "${name}")
    if(NOT files)
        message(WARNING "no license file found for ${name} in ${root}")
        string(APPEND sections "\nNo license file was found in the source of this library when the host was built.\n")
    endif()
    foreach(file IN LISTS files)
        file(READ "${file}" body)
        string(STRIP "${body}" body)
        get_filename_component(file_name "${file}" NAME)
        string(APPEND sections "\n${file_name}:\n\n${body}\n")
    endforeach()
endforeach()

if(names)
    list(SORT names)
    list(JOIN names "\n  " listed)
    string(APPEND text "\
${rule}
Libraries built into Scratch Everywhere!

The Scratch Everywhere! library contains these libraries, whose own notices follow:

  ${listed}
${sections}")
else()
    message(WARNING "the libraries built into Scratch Everywhere! could not be listed")
    string(APPEND text "\
${rule}
Libraries built into Scratch Everywhere!

Scratch Everywhere! is built from other libraries, which could not be listed when this host was
built. Their notices have to be added by hand.
")
endif()

file(WRITE "${OUT}" "${text}")
