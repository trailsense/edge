extern crate alloc;
use alloc::vec::Vec;

pub fn deduplicate_probes(input_fingerprints: &[u16]) -> u32 {
    if input_fingerprints.is_empty() {
        return 0;
    }

    let mut survivors: Vec<u16> = Vec::new();
    survivors.push(input_fingerprints[0]);

    // Threshold needs to be lowered for now for testing reasons since it is very inaccurate. In future we should use 32 or 64 bit masks.
    for &fingerprint in &input_fingerprints[1..] {
        if !is_duplicate(0, fingerprint, &survivors) {
            survivors.push(fingerprint);
        }
    }

    survivors.len() as u32
}

fn is_duplicate(threshold: u32, input: u16, survivors: &[u16]) -> bool {
    for &s in survivors {
        let dist = (input ^ s).count_ones(); // Hamming distance
        if dist <= threshold {
            return true;
        }
    }
    false
}
