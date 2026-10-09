# mbedTLS built for the computer, and the tests of the firmware's TLS code that run against it.
#   cmake --preset host            (from this directory; or `make -C esphome host-tls`, or CMake Tools in VS Code)
# MBEDTLS_SRC is found by itself in ESPHome's cache once the firmware has been compiled; give it only to use another.
cmake_minimum_required(VERSION 3.16)
project(eink_host_tls C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_C_STANDARD 11)

set(MBEDTLS_SRC "" CACHE PATH "mbedTLS source: the one inside the ESP-IDF that ESPHome builds with")
if(NOT MBEDTLS_SRC)
  # The ESP-IDF ESPHome fetched when it compiled the firmware: macOS keeps it under Library/Caches, Linux under .cache.
  # The newest version if there are several.
  file(GLOB candidates
       "$ENV{HOME}/Library/Caches/esphome/idf/frameworks/*/components/mbedtls/mbedtls"
       "$ENV{HOME}/.cache/esphome/idf/frameworks/*/components/mbedtls/mbedtls")
  list(SORT candidates COMPARE NATURAL ORDER DESCENDING)
  if(candidates)
    list(GET candidates 0 found)
    set(MBEDTLS_SRC "${found}" CACHE PATH "" FORCE)
  endif()
endif()
if(NOT EXISTS "${MBEDTLS_SRC}/include/mbedtls/ssl.h")
  message(FATAL_ERROR
    "mbedTLS source not found (MBEDTLS_SRC='${MBEDTLS_SRC}'). It is in the ESP-IDF that ESPHome fetches when it compiles "
    "the firmware: run `make -C esphome compile` once, or pass -DMBEDTLS_SRC=<ESP-IDF>/components/mbedtls/mbedtls.")
endif()
message(STATUS "mbedTLS from ${MBEDTLS_SRC}")

# For the editor: IntelliSense for these programs from the exact commands that build them.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

set(ENABLE_PROGRAMS OFF CACHE BOOL "" FORCE)
set(ENABLE_TESTING OFF CACHE BOOL "" FORCE)
set(MBEDTLS_FATAL_WARNINGS OFF CACHE BOOL "" FORCE)
set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -DMBEDTLS_CONFIG_FILE=\\\"${CMAKE_CURRENT_SOURCE_DIR}/mbedtls_host_config.h\\\"")
add_subdirectory(${MBEDTLS_SRC} mbedtls EXCLUDE_FROM_ALL)

set(FIRMWARE ${CMAKE_CURRENT_SOURCE_DIR}/../..)

# Everything here is built against the same mbedTLS and the firmware's own headers.
function(host_target name)
  add_executable(${name} ${ARGN})
  target_include_directories(${name} PRIVATE ${FIRMWARE} ${FIRMWARE}/tests/support)
  target_compile_definitions(${name} PRIVATE MBEDTLS_CONFIG_FILE="${CMAKE_CURRENT_SOURCE_DIR}/mbedtls_host_config.h")
  target_compile_options(${name} PRIVATE -Wall -Wextra -Wno-unused-parameter)
  target_link_libraries(${name} PRIVATE mbedtls mbedx509 mbedcrypto)
endfunction()

# The firmware's own code (core/join.h over tls/est_client.h and the rest), against a real server.
host_target(join_host_test join_host_test.cpp ${FIRMWARE}/tests/support/main.cpp)
host_target(stream_host_test stream_host_test.cpp ${FIRMWARE}/tests/support/main.cpp)
host_target(begin_host_test begin_host_test.cpp ${FIRMWARE}/tests/support/main.cpp)
host_target(resume_host_test resume_host_test.cpp ${FIRMWARE}/tests/support/main.cpp)

# The tests against a real server, for ctest and the editor's test panel. Each test is run by itself against its own server
# (with_fixture.sh), since the server limits how many different display names one address may join with in a day, and the
# programs run side by side (run_host_tests.sh). This is one entry that runs them all through the same Makefile target a
# person would, and reports what failed.
enable_testing()
add_test(NAME host-tests COMMAND make -C ${FIRMWARE} host-test)
set_tests_properties(host-tests PROPERTIES TIMEOUT 300)
