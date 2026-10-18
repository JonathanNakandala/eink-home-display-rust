// THROWAWAY (bench): puts the SystemView trace on GPIO47 (out) and GPIO6 (in), which the Kconfig can't name: its pin range
// stops at 46 and quietly falls back to the default, GPIO12, which is the display controller's reset line.
#pragma once
#include "esp_app_trace.h"
#include "esp_trace.h"

extern "C" esp_trace_open_params_t esp_trace_get_user_params(void) {
  static esp_apptrace_config_t config;
  config = {};
  config.dest = ESP_APPTRACE_DEST_UART;
  config.dest_cfg.uart.uart_num = CONFIG_APPTRACE_DEST_UART_NUM;
  config.dest_cfg.uart.tx_pin_num = 47;
  config.dest_cfg.uart.rx_pin_num = 6;
  config.dest_cfg.uart.baud_rate = CONFIG_APPTRACE_UART_BAUDRATE;
  config.dest_cfg.uart.tx_buff_size = CONFIG_APPTRACE_UART_TX_BUFF_SIZE;
  config.dest_cfg.uart.tx_msg_size = CONFIG_APPTRACE_UART_TX_MSG_SIZE;
  config.flush_tmo = CONFIG_APPTRACE_ONPANIC_HOST_FLUSH_TMO;
  config.flush_thresh = CONFIG_APPTRACE_POSTMORTEM_FLUSH_THRESH;
  esp_trace_open_params_t params = {};
  params.encoder_name = "sysview";
  params.transport_name = CONFIG_ESP_TRACE_TRANSPORT_NAME;
  params.transport_cfg = &config;
  return params;
}
