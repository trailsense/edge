# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-04-16

### Initial release
- ESP32 edge node foundation with GSM uplink support and orchestrated node lifecycle.
- Core pipeline for collecting counts/fingerprints and sending ingest payloads.
- Retry/recovery behavior for network/connectivity failures.

### Testing
- Host test suite for pure logic in `tests/host`.
- Mandatory property test coverage in host CI.
- Local ESP32 serial lifecycle validation in `tests/device` (`run-modem-lifecycle.sh`).

### CI and workflow
- GitHub Actions host tests on pull requests.
- Device validation defined as local hardware run before merge.
- PR template includes required host/device validation checklist.

### Notes
- `TRAILSENSE_API_URL` and `TRAILSENSE_EDGE_ID` are compile-time configuration inputs.
- Device tests are local-only and require a connected ESP32.
- Data persistence/eviction hardening is tracked separately for a follow-up release.
