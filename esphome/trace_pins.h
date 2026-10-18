// THROWAWAY (bench): puts the SystemView trace on GPIO47 (out) and GPIO6 (in), which the Kconfig can't name: its pin range
// stops at 46 and quietly falls back to the default, GPIO12, which is the display controller's reset line.
#pragma once
#include "esp_app_trace.h"
#include "esp_trace.h"
#include "SEGGER_SYSVIEW.h"
#include "soc/uart_struct.h"

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

#include "esp_timer.h"
#include "esp_rom_sys.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

// Prints each task's state with the ROM printer, which takes no lock, so it still speaks if the logger is blocked.
inline void trace_watch_tick(void *) {
  esp_rom_printf("[watch] tick\n");
  static char list[800];
  vTaskList(list);
  esp_rom_printf("\n[watch] uart1_rx=%u started=%d\n%s", (unsigned) UART1.status.rxfifo_cnt, (int) SEGGER_SYSVIEW_IsStarted(), list);
}
inline void trace_watch_start() {
  esp_rom_printf("[watch] start\n");
  static esp_timer_handle_t timer;
  const esp_timer_create_args_t args = {.callback = trace_watch_tick, .name = "watch"};
  esp_timer_create(&args, &timer);
  esp_timer_start_periodic(timer, 3000000);
}
