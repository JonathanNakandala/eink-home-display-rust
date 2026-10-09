# mbedTLS built for the computer, and the tests of the firmware's TLS code that run against it.
#   cmake -S host -B build/host -DMBEDTLS_SRC=<ESP-IDF>/components/mbedtls/mbedtls
cmake_minimum_required(VERSION 3.16)
project(eink_host_tls C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_C_STANDARD 11)

set(MBEDTLS_SRC "" CACHE PATH "mbedTLS source: the one inside the ESP-IDF that ESPHome builds with")
if(NOT EXISTS "${MBEDTLS_SRC}/include/mbedtls/ssl.h")
  message(FATAL_ERROR "MBEDTLS_SRC does not name an mbedTLS source tree: ${MBEDTLS_SRC}")
endif()

set(ENABLE_PROGRAMS OFF CACHE BOOL "" FORCE)
set(ENABLE_TESTING OFF CACHE BOOL "" FORCE)
set(MBEDTLS_FATAL_WARNINGS OFF CACHE BOOL "" FORCE)
set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -DMBEDTLS_CONFIG_FILE=\\\"${CMAKE_CURRENT_SOURCE_DIR}/mbedtls_host_config.h\\\"")
add_subdirectory(${MBEDTLS_SRC} mbedtls EXCLUDE_FROM_ALL)

set(FIRMWARE ${CMAKE_CURRENT_SOURCE_DIR}/..)

# Everything here is built against the same mbedTLS and the firmware's own headers.
function(host_target name)
  add_executable(${name} ${ARGN})
  target_include_directories(${name} PRIVATE ${FIRMWARE} ${FIRMWARE}/tests)
  target_compile_definitions(${name} PRIVATE MBEDTLS_CONFIG_FILE="${CMAKE_CURRENT_SOURCE_DIR}/mbedtls_host_config.h")
  target_compile_options(${name} PRIVATE -Wall -Wextra -Wno-unused-parameter)
  target_link_libraries(${name} PRIVATE mbedtls mbedx509 mbedcrypto)
endfunction()

# The first probe, kept: the whole conversation written out step by step.
host_target(spike_tls spike_tls.cpp)

# The firmware's own code (eink_join.h over eink_est_client.h and the rest), against a real server.
host_target(join_host_test join_host_test.cpp ${FIRMWARE}/tests/main.cpp)
host_target(stream_host_test stream_host_test.cpp ${FIRMWARE}/tests/main.cpp)
host_target(begin_host_test begin_host_test.cpp ${FIRMWARE}/tests/main.cpp)
