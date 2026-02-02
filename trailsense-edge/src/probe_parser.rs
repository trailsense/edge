extern crate alloc;
use crate::models::MODEL;

/// # Fingerprint Probe
///
/// Generate a fingerprint for the given probe data using the defined filters generated with a python script.
/// Each filter outputs a single bit, which are concatenated to form the final fingerprint.
///
/// # Arguments
///
/// * `data` - A byte slice representing the probe data to be fingerprinted.
///
/// # Returns
///
/// A `u16` value is returned, where each bit represents one bit of the filter.
pub fn fingerprint_probe(data: &[u8]) -> u16 {
    // Change to u32 or as needed if increasing filter size.
    let mut fingerprint = 0u16;

    for (idx, model) in MODEL.iter().enumerate() {
        let max_iterations = core::cmp::min(data.len(), model.mask.len());

        let mut xor_result = 0u8;

        for i in 0..max_iterations {
            if model.mask[i] != 0x00 {
                xor_result ^= data[i]
            }
        }

        let bit = (xor_result.count_ones() % 2) as u16;
        log::info!("filter {} -> xor={:#04x}, bit={}", idx, xor_result, bit);
        fingerprint = (fingerprint << 1) | bit;
    }

    fingerprint
}
