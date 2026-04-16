#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET_BIN="${ROOT_DIR}/target/xtensa-esp32-none-elf/release/modem_lifecycle_serial"
LOG_FILE="$(mktemp -t trailsense-device-test.XXXXXX.log)"
TEST_TIMEOUT_SECS="${TEST_TIMEOUT_SECS:-300}"
PASS_MARKER="DEVICE_TEST_PASS modem_lifecycle_serial"
FAIL_MARKER="DEVICE_TEST_FAIL modem_lifecycle_serial"

cleanup() {
  if [[ -n "${FLASH_PID:-}" ]] && kill -0 "${FLASH_PID}" 2>/dev/null; then
    kill "${FLASH_PID}" 2>/dev/null || true
    wait "${FLASH_PID}" 2>/dev/null || true
  fi
}

trap cleanup EXIT

has_marker() {
  local marker="$1"

  if command -v rg >/dev/null 2>&1; then
    rg -q --fixed-strings "${marker}" "${LOG_FILE}"
  else
    grep -Fq "${marker}" "${LOG_FILE}"
  fi
}

echo "Building serial device lifecycle binary..."
cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --release --bin modem_lifecycle_serial

echo "Flashing and monitoring ESP32 (timeout: ${TEST_TIMEOUT_SECS}s)..."
if [[ -n "${ESPFLASH_PORT:-}" ]]; then
  (
    espflash flash --chip esp32 --monitor --non-interactive --monitor-baud 115200 -p "${ESPFLASH_PORT}" "${TARGET_BIN}" 2>&1 \
      | tee "${LOG_FILE}"
  ) &
else
  (
    espflash flash --chip esp32 --monitor --non-interactive --monitor-baud 115200 "${TARGET_BIN}" 2>&1 \
      | tee "${LOG_FILE}"
  ) &
fi

FLASH_PID=$!
END_TIME=$((SECONDS + TEST_TIMEOUT_SECS))

while (( SECONDS < END_TIME )); do
  if has_marker "${PASS_MARKER}"; then
    echo "Device lifecycle test passed."
    exit 0
  fi

  if has_marker "${FAIL_MARKER}"; then
    echo "Device lifecycle test failed."
    exit 1
  fi

  if ! kill -0 "${FLASH_PID}" 2>/dev/null; then
    break
  fi

  sleep 1
done

if has_marker "${PASS_MARKER}"; then
  echo "Device lifecycle test passed."
  exit 0
fi

if has_marker "${FAIL_MARKER}"; then
  echo "Device lifecycle test failed."
  exit 1
fi

echo "Device lifecycle test timed out or exited without PASS marker."
exit 1
