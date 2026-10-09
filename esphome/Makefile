# Tests for the calculations in the headers that don't touch hardware (core/wake.h, core/battery.h,
# core/format.h, core/report.h, core/clock.h). They need only a C++17 compiler: `make -C esphome test`.
# Undefined-behaviour and address sanitizers are on, so a signed overflow or a bad index fails the run
# instead of passing by luck. The headers that need the device (esp/telemetry.h, esp/discovery.h,
# esp/health.h, esp/notice.h) are not built here; `esphome compile` covers them.
CXX ?= c++
CXXFLAGS ?= -std=c++17 -I. -Itests/support -Wall -Wextra -Werror -O1 -g -fsanitize=address,undefined -fno-sanitize-recover=undefined
SOURCES := tests/support/main.cpp $(wildcard tests/core/*_test.cpp)
HEADERS := $(wildcard eink/core/*.h) $(wildcard tests/support/*.h)

.PHONY: test
test: build/tests
	./build/tests

build/tests: $(SOURCES) $(HEADERS)
	@mkdir -p build
	$(CXX) $(CXXFLAGS) -o $@ $(SOURCES)

# The firmware build tools are pinned in ../pyproject.toml and installed into ../.venv by Poetry (`poetry install` at
# the repository root). This is not a Python project: Poetry is only how ESPHome gets installed.
POETRY ?= poetry
YAML := reterminal-e1003.yaml

# secrets.yaml is git-ignored; `config` and `compile` need one to exist, so an example one stands in for checking.
secrets.yaml:
	cp secrets.example.yaml $@

# Checks the YAML (every component, every !lambda's syntax of the YAML around it) in seconds. No toolchain download.
.PHONY: check
check: secrets.yaml
	$(POETRY) run yamllint -c ../.yamllint.yaml $(YAML) packages
	$(POETRY) run esphome config $(YAML) > /dev/null

# Builds the firmware, which is what catches errors in the C++ of the lambdas and the device headers. The first run
# fetches PlatformIO and the ESP-IDF toolchain (a few GB); later ones are incremental.
.PHONY: compile
compile: secrets.yaml
	$(POETRY) run esphome compile $(YAML)

# The C++ is held to .clang-format, and the helper script to ruff (configured in ../pyproject.toml), which is the style it is already written in. `lint` fails on a difference and
# `format` fixes them. The headers that need ESPHome are formatted too, but only `compile` can build them.
CPP := $(shell find eink -name '*.h') $(wildcard tests/core/*.cpp) $(wildcard tests/tls/*.cpp) $(wildcard tests/support/*.h)

.PHONY: lint
lint:
	$(POETRY) run clang-format --dry-run -Werror $(CPP)
	$(POETRY) run ruff check scripts
	$(POETRY) run ruff format --check scripts
	python3 scripts/check_layers.py

.PHONY: format
format:
	$(POETRY) run clang-format -i $(CPP)
	$(POETRY) run ruff format scripts

# Writes .vscode/c_cpp_properties.json from the compiler command the last `compile` recorded, so the editor finds
# ESPHome's and ESP-IDF's headers. The file holds paths on this machine and is git-ignored. Pick the configuration
# in VS Code's status bar: "ESPHome firmware" for the device headers, "Host tests" for tests/.
.PHONY: vscode
vscode:
	$(POETRY) run python scripts/vscode_intellisense.py

# mbedTLS built for the computer, from the very source ESPHome builds the chip's with (the ESP-IDF in its cache, so
# `compile` has to have run once), and the spike that runs the display's whole TLS conversation against the real server.
# Needs cmake and cargo. See tests/tls/README.md.
MBEDTLS_SRC ?= $(firstword $(wildcard $(HOME)/Library/Caches/esphome/idf/frameworks/*/components/mbedtls/mbedtls $(HOME)/.cache/esphome/idf/frameworks/*/components/mbedtls/mbedtls))

.PHONY: host-tls
host-tls:
	@test -n "$(MBEDTLS_SRC)" || { echo "mbedTLS source not found: run 'make compile' first, or set MBEDTLS_SRC" >&2; exit 1; }
	cmake -S tests/tls -B build/host -DMBEDTLS_SRC=$(MBEDTLS_SRC) -DCMAKE_BUILD_TYPE=Debug > /dev/null
	cmake --build build/host --target spike_tls join_host_test stream_host_test begin_host_test resume_host_test

.PHONY: spike
spike: host-tls
	tests/tls/run_spike.sh

# The firmware's joining code over its own TLS and EST code, run against a real server (needs cmake and cargo).
.PHONY: host-test
host-test: host-tls host-programs
	FIXTURE_PROGRAM=$(abspath build/host/bin/est_fixture) DISPLAYCTL_PROGRAM=$(abspath build/host/bin/displayctl) \
		tests/tls/run_host_tests.sh build/host join_host_test stream_host_test begin_host_test resume_host_test

# The test server and displayctl, built once and kept where cargo does not replace them (tests/tls/stage_programs.sh says why).
.PHONY: host-programs
host-programs:
	tests/tls/stage_programs.sh build/host/bin

.PHONY: clean
clean:
	rm -rf build
