# Tests for the calculations in the headers that don't touch hardware (eink_wake.h, eink_battery.h,
# eink_format.h, eink_report.h). They need only a C++17 compiler: `make -C esphome test`.
# Undefined-behaviour and address sanitizers are on, so a signed overflow or a bad index fails the run
# instead of passing by luck. The headers that need the device (eink_telemetry.h, eink_discovery.h,
# eink_health.h, eink_notice.h) are not built here; `esphome compile` covers them.
CXX ?= c++
CXXFLAGS ?= -std=c++17 -Wall -Wextra -Werror -O1 -g -fsanitize=address,undefined -fno-sanitize-recover=undefined
SOURCES := tests/main.cpp $(wildcard tests/*_test.cpp)
HEADERS := eink_wake.h eink_battery.h eink_format.h eink_report.h tests/check.h

.PHONY: test
test: build/tests
	./build/tests

build/tests: $(SOURCES) $(HEADERS)
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -o $@ $(SOURCES)

.PHONY: clean
clean:
	rm -rf build
