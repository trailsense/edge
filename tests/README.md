# Tests

This folder contains host tests and local ESP32 device tests.

## Host tests

Run:

```bash
TRAILSENSE_API_URL=https://api.trailsense.daugt.com \
TRAILSENSE_EDGE_ID=ci-test-node \
cargo test --manifest-path tests/host/Cargo.toml --features property-tests
```

Or export once in your shell session:

```bash
export TRAILSENSE_API_URL=https://api.trailsense.daugt.com
export TRAILSENSE_EDGE_ID=ci-test-node
cargo test --manifest-path tests/host/Cargo.toml --features property-tests
```

CI:

- host tests run in GitHub Actions on pull requests and on `main` pushes.
- property tests are mandatory in CI.

## Device tests (local only)

Run on a connected ESP32:

```bash
cd tests/device
./run-modem-lifecycle.sh
```

If port auto-detection is unstable:

```bash
ESPFLASH_PORT=/dev/cu.usbserial-210 ./run-modem-lifecycle.sh
```

Requirements:

- ESP32 connected by USB
- SIM/network available for GSM path
- `espflash` installed

Note:

- probe-based `embedded-test` flow is not active in this project right now.
- device tests are local-only before merge (no device GitHub Action).
