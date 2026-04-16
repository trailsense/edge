#![allow(clippy::panic)]

use atat::{AtatCmd, Parser};

#[path = "../../../trailsense-edge/src/network/config.rs"]
mod config;
#[path = "../../../trailsense-edge/src/network/gsm/commands.rs"]
mod gsm_commands;
#[path = "../../../trailsense-edge/src/network/gsm/recovery.rs"]
mod gsm_recovery;
#[path = "../../../trailsense-edge/src/network/types.rs"]
mod network_types;
#[path = "../../../trailsense-edge/src/orchestration/policy.rs"]
mod orchestration_policy;
#[path = "../../../trailsense-edge/src/probes/counter.rs"]
mod probe_counter;

#[test]
fn deduplicate_empty_returns_zero() {
    assert_eq!(probe_counter::deduplicate_probes(&[]), 0);
}

#[test]
fn deduplicate_counts_unique_fingerprints() {
    let input = [0x0001, 0x0002, 0x0003, 0x0004];
    assert_eq!(probe_counter::deduplicate_probes(&input), 4);
}

#[test]
fn deduplicate_filters_exact_duplicates() {
    let input = [0x00AA, 0x00AA, 0x00AA, 0x00AB, 0x00AB, 0x00AC];
    assert_eq!(probe_counter::deduplicate_probes(&input), 3);
}

#[cfg(feature = "property-tests")]
mod property_tests {
    use super::*;
    use quickcheck::quickcheck;

    quickcheck! {
        fn deduplicate_output_is_bounded(values: Vec<u16>) -> bool {
            probe_counter::deduplicate_probes(&values) as usize <= values.len()
        }
    }
}

#[test]
fn ingest_url_builds_expected_suffix() {
    let url = config::ingest_url().expect("ingest URL should be buildable");
    assert_eq!(url.as_str(), "https://api.trailsense.daugt.com/ingest");
}

#[test]
fn package_dto_serializes_with_expected_fields() {
    let dto = network_types::PackageDto::new(42, 7, "edge-node");
    let json = serde_json::to_string(&dto).expect("serialization should succeed");
    assert_eq!(
        json,
        "{\"age_in_seconds\":42,\"count\":7,\"node_id\":\"edge-node\"}"
    );
}

#[test]
fn raw_at_cmd_appends_crlf() {
    let cmd = gsm_commands::RawAtCmd::<64, 1_000>::new("AT+CRESET");
    let mut buf = [0u8; 64];
    let used = cmd.write(&mut buf);

    assert_eq!(used, "AT+CRESET\r\n".len());
    assert_eq!(&buf[..used], b"AT+CRESET\r\n");
}

#[test]
fn raw_payload_is_written_without_trailer() {
    let cmd = gsm_commands::RawPayload::<64, 1_000>::new("{\"ok\":true}");
    let mut buf = [0u8; 64];
    let used = cmd.write(&mut buf);

    assert_eq!(used, "{\"ok\":true}".len());
    assert_eq!(&buf[..used], b"{\"ok\":true}");
}

#[test]
fn raw_at_read_cmd_parses_utf8_response() {
    let cmd = gsm_commands::RawAtReadCmd::<64, 1_000>::new("AT+IPADDR");
    let parsed = cmd
        .parse(Ok(b"+IPADDR: 10.0.0.1\r\n"))
        .expect("parse should work");
    assert_eq!(parsed.as_str(), "+IPADDR: 10.0.0.1\r\n");
}

#[test]
fn http_urc_parser_accepts_complete_line() {
    let input = b"\r\n+HTTPACTION: 1,200,13\r\nREST";
    let (line, consumed) = gsm_commands::HttpUrcParser::parse(input).expect("should parse");

    assert_eq!(line, b"+HTTPACTION: 1,200,13");
    assert_eq!(consumed, 25);
}

#[test]
fn http_urc_parser_reports_incomplete_prefix() {
    let input = b"\r\n+HTTPACT";
    assert!(matches!(
        gsm_commands::HttpUrcParser::parse(input),
        Err(atat::digest::ParseError::Incomplete)
    ));
}

#[test]
fn http_urc_parser_rejects_other_urcs() {
    let input = b"\r\n+CEREG: 0,1\r\n";
    assert!(matches!(
        gsm_commands::HttpUrcParser::parse(input),
        Err(atat::digest::ParseError::NoMatch)
    ));
}

#[test]
fn retry_classification_marks_transient_and_hard_errors() {
    assert_eq!(
        gsm_commands::GsmError::IpTimeout.kind(),
        gsm_commands::GsmErrorKind::Transient
    );
    assert_eq!(
        gsm_commands::GsmError::HttpStatus(503).kind(),
        gsm_commands::GsmErrorKind::Transient
    );
    assert_eq!(
        gsm_commands::GsmError::HttpStatus(400).kind(),
        gsm_commands::GsmErrorKind::Hard
    );
    assert_eq!(
        gsm_commands::GsmError::BufferTooSmall {
            needed: 100,
            available: 32,
        }
        .kind(),
        gsm_commands::GsmErrorKind::Hard
    );
}

#[test]
fn recovery_stage_policy_matches_expected_round_robin() {
    const UPLOAD_RECOVERY_THRESHOLD: u8 = 3;

    let connect = (1u8..=7)
        .map(gsm_recovery::connect_recovery_stage_index)
        .map(|stage| stage + 1)
        .collect::<Vec<_>>();
    assert_eq!(connect, vec![1, 2, 3, 1, 2, 3, 1]);

    let upload = (1u8..=8)
        .map(|streak| {
            gsm_recovery::upload_recovery_stage_index(streak, UPLOAD_RECOVERY_THRESHOLD)
                .map(|stage| stage + 1)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        upload,
        vec![
            None,
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(1),
            Some(2),
            Some(3)
        ]
    );
}

#[test]
fn network_failure_policy_falls_back_at_limit() {
    assert!(!orchestration_policy::should_fallback_to_saving(4));
    assert!(orchestration_policy::should_fallback_to_saving(5));
}

#[test]
fn save_policy_enters_sleep_at_thresholds() {
    assert!(!orchestration_policy::should_enter_sleep(9, 4));
    assert!(orchestration_policy::should_enter_sleep(10, 0));
    assert!(orchestration_policy::should_enter_sleep(0, 5));
}

#[test]
fn counter_policy_uses_saturating_increment() {
    assert_eq!(orchestration_policy::bump_counter(0), 1);
    assert_eq!(orchestration_policy::bump_counter(u8::MAX), u8::MAX);
}
