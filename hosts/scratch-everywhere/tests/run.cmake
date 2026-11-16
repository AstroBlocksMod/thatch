# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Runs the test game for two seconds, plays the input script and compares the bubbles the game
# prints with expected.txt. The game was built by `thatch build --host scratch-everywhere`, so it
# finds its project file and the shared library next to itself; this also checks the license
# notices that build wrote. Called by ctest with -DGAME, -DNOTICES, -DINPUT and -DEXPECTED.

execute_process(
    COMMAND "${GAME}" --frames 60 --seed 1 --input "${INPUT}" --print-bubbles
    OUTPUT_VARIABLE output
    ERROR_VARIABLE errors
    RESULT_VARIABLE status
    TIMEOUT 120)
if(NOT status EQUAL 0)
    message(FATAL_ERROR "the game failed with status ${status}:\n${output}\n${errors}")
endif()

# The library logs to standard output as well; the bubbles are the lines the adapter printed.
string(REGEX MATCHALL "[^\n]* (says|thinks): [^\n]*\n" bubbles "${output}")
string(JOIN "" bubbles ${bubbles})
file(READ "${EXPECTED}" expected)
if(NOT bubbles STREQUAL expected)
    message(FATAL_ERROR "the game printed:\n${bubbles}\nbut expected.txt has:\n${expected}\nfull output:\n${output}\n${errors}")
endif()

# The notices next to the game: the runtime's, then what this host adds.
file(READ "${NOTICES}" notices)
foreach(wanted
        "The Rust standard library"
        "This program uses Scratch Everywhere!"
        "GNU LESSER GENERAL PUBLIC LICENSE"
        "GNU GENERAL PUBLIC LICENSE"
        "The change to Scratch Everywhere!"
        "Libraries built into Scratch Everywhere!")
    string(FIND "${notices}" "${wanted}" at)
    if(at EQUAL -1)
        message(FATAL_ERROR "${NOTICES} does not have \"${wanted}\"")
    endif()
endforeach()
