# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Runs the test game for two seconds, plays the input script and compares the bubbles the game
# prints with expected.txt. Called by ctest with -DGAME, -DPROJECT, -DINPUT and -DEXPECTED.

execute_process(
    COMMAND "${GAME}" "${PROJECT}" --frames 60 --seed 1 --input "${INPUT}" --print-bubbles
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
