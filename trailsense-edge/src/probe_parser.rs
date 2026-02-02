extern crate alloc;
use crate::models::{MODEL, WeakClassifier};
use alloc::vec::Vec;

pub fn fingerprint_probe(data: &[u8]) -> Vec<u8> {
    let mut fingerprint = Vec::<u8>::new();
    for model in MODEL {
        let mut xor_result = 0u8;
        for i in 0..data.len() {
            if model.mask[i] != 0x00 {
                xor_result ^= data[i]
            }
        }

        let bit = (xor_result.count_ones() % 2) as u8;
        fingerprint.push(bit);
    }

    fingerprint
}
