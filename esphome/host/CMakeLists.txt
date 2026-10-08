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

add_executable(spike_tls spike_tls.cpp)
target_include_directories(spike_tls PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_definitions(spike_tls PRIVATE MBEDTLS_CONFIG_FILE="${CMAKE_CURRENT_SOURCE_DIR}/mbedtls_host_config.h")
target_link_libraries(spike_tls PRIVATE mbedtls mbedx509 mbedcrypto)
