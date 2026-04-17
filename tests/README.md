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

- host tests run in GitHub Actions on pull requests.
- property tests are mandatory in CI.

### Dataset #2 single-device accuracy check

To evaluate dedup/fingerprint behavior on the labeled per-device captures in:
`tests/training_data/Individual devices`

run:

```bash
cargo run --manifest-path tests/host/Cargo.toml --bin evaluate_dataset
```

Optional:

```bash
# Evaluate at a custom threshold without editing models.rs
cargo run --manifest-path tests/host/Cargo.toml --bin evaluate_dataset -- --tau 43.35

# Sweep around current tau and print best values
cargo run --manifest-path tests/host/Cargo.toml --bin evaluate_dataset -- --sweep-tau

# Optimize tau for production counting:
# - single-device files (expected count=1)
# - synthetic 2-device and 3-device windows
# - held-out channel folds (1/6/11)
# Objective balances false merges (undercount) and false non-merges (overcount).
cargo run --manifest-path tests/host/Cargo.toml --bin optimize_tau

# These 2 commands are only a sanity check: they confirm Python and Rust produce the same fingerprint bits.
# Export a Rust-computed parity fixture (raw probe bytes + Rust fingerprints + model)
cargo run --manifest-path tests/host/Cargo.toml --bin export_parity_fixture -- --out tests/host/parity_fixture.json --max-samples 500

# Recompute in Python and assert bit-level equality with Rust
python3 tests/host/tools/check_parity.py tests/host/parity_fixture.json
```

This prints:

- overall single-device accuracy (`dedup_count == 1`)
- mean absolute count error vs expected count `1`
- per-device and per-mode breakdowns
- worst outlier files and zero-probe files

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
