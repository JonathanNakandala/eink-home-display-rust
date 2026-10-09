/* The mbedTLS configuration for building the library on a computer, to test the firmware's TLS code against the same
 * library the chip runs (mbedTLS 3.6.x, from the ESP-IDF that ESPHome builds with).
 *
 * It starts from upstream's defaults, which already have TLS 1.3, session tickets and CSR writing, and adds what the
 * chip's build (esphome/.esphome/build, sdkconfig) has to turn on for the firmware too: the keying-material exporter
 * that RFC 9266 channel binding needs. It does not copy the chip's whole configuration: the chip's hardware
 * acceleration and its smaller buffers do not exist here, so heap and speed are not measured by anything built from this.
 */
#include "mbedtls/mbedtls_config.h"

#define MBEDTLS_SSL_KEYING_MATERIAL_EXPORT
