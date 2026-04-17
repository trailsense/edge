extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use log::error;

use crate::probes::models::{MODEL, MODEL_SIZE, TAU};

static INVALID_MODEL_LOGGED: AtomicBool = AtomicBool::new(false);

pub fn deduplicate_probes(input_fingerprints: &[u64]) -> u32 {
    if input_fingerprints.is_empty() {
        return 0;
    }

    if !validate_runtime_model_config() {
        return input_fingerprints.len() as u32;
    }

    let mut survivors: Vec<u64> = Vec::new();
    survivors.push(input_fingerprints[0]);

    for &fingerprint in &input_fingerprints[1..] {
        if !is_duplicate(fingerprint, &survivors) {
            survivors.push(fingerprint);
        }
    }

    survivors.len() as u32
}

fn is_duplicate(input: u64, survivors: &[u64]) -> bool {
    survivors.iter().any(|&s| weighted_score(input, s) >= TAU)
}

fn weighted_score(a: u64, b: u64) -> f32 {
    let mut score = 0.0_f32;

    for i in 0..MODEL_SIZE {
        // Important: map model[i] to the correct fingerprint bit position.
        // First filter ends up in bit 63, last in bit 0 (for MODEL_SIZE=64).
        let bit_pos = MODEL_SIZE - 1 - i;
        let mask = 1u64 << bit_pos;

        if (a & mask) == (b & mask) {
            score += MODEL[i].alpha;
        } else {
            score -= MODEL[i].alpha;
        }
    }

    score
}

pub fn validate_runtime_model_config() -> bool {
    if MODEL_SIZE == 0 || MODEL_SIZE > 64 {
        log_invalid_model_once("DEDUP config invalid: MODEL_SIZE must be in 1..=64");
        return false;
    }

    if !TAU.is_finite() {
        log_invalid_model_once("DEDUP config invalid: TAU must be finite");
        return false;
    }

    let mut alpha_sum = 0.0_f32;
    for (idx, model) in MODEL.iter().enumerate() {
        if !model.alpha.is_finite() {
            log_invalid_model_once("DEDUP config invalid: alpha must be finite for all filters");
            error!(
                "DEDUP config invalid: alpha is non-finite at filter {}",
                idx
            );
            return false;
        }
        if model.alpha <= 0.0 {
            log_invalid_model_once("DEDUP config invalid: alpha must be > 0 for all filters");
            error!("DEDUP config invalid: alpha <= 0 at filter {}", idx);
            return false;
        }
        alpha_sum += model.alpha;
    }

    // For +/- alpha scoring, total score is bounded to [-sum(alpha), +sum(alpha)].
    if TAU < -alpha_sum || TAU > alpha_sum {
        log_invalid_model_once(
            "DEDUP config invalid: TAU is outside reachable score range [-sum(alpha), +sum(alpha)]",
        );
        error!(
            "DEDUP config invalid: TAU={} but reachable range is [{}, {}]",
            TAU, -alpha_sum, alpha_sum
        );
        return false;
    }

    true
}

fn log_invalid_model_once(msg: &str) {
    if !INVALID_MODEL_LOGGED.swap(true, Ordering::Relaxed) {
        error!("{}", msg);
    }
}
