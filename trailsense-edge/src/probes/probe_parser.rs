extern crate alloc;
use esp_radio::wifi::PromiscuousPkt;
use ieee80211::{
    common::{FrameControlField, FrameType, ManagementFrameSubtype},
    mgmt_frame::{ProbeRequestFrame, body::HasElements},
    scroll::Pread,
};
use log::warn;

use crate::probes::{fingerprint_store, models::MODEL};

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
fn fingerprint_probe(data: &[u8]) -> u16 {
    // Change to u32 or as needed if increasing filter size (with u32, 32 filters are usable).
    let mut fingerprint = 0u16;

    for (idx, model) in MODEL.iter().enumerate() {
        let max_iterations = core::cmp::min(
            data.len(),
            core::cmp::min(model.positive_mask.len(), model.negative_mask.len()),
        );
        let mut score: i32 = 0;
        for i in 0..max_iterations {
            let positive_bits = data[i] & model.positive_mask[i];
            let negative_bits = data[i] & model.negative_mask[i];

            // Debug assertion to catch any mask generation errors during development.
            // The masks are designed to be disjoint, so this should never trigger.
            debug_assert_eq!(
                model.positive_mask[i] & model.negative_mask[i],
                0,
                "Mask overlap detected at filter {} position {}: positive_mask={:#x}, negative_mask={:#x}",
                idx,
                i,
                model.positive_mask[i],
                model.negative_mask[i]
            );

            score += positive_bits.count_ones() as i32;
            score -= negative_bits.count_ones() as i32;
        }

        let bit = if score >= model.threshold as i32 {
            1
        } else {
            0
        };
        fingerprint = (fingerprint << 1) | bit;
    }
    if !fingerprint_store::push(fingerprint) {
        warn!("Fingerprint overflow!");
    }
    fingerprint
}

pub fn read_packet(packet: PromiscuousPkt<'_>) {
    if packet.data.len() < 2 {
        return;
    }

    let fcf_bits = u16::from_le_bytes([packet.data[0], packet.data[1]]);
    let fcf = FrameControlField::from_bits(fcf_bits);
    if !matches!(
        fcf.frame_type(),
        FrameType::Management(ManagementFrameSubtype::ProbeRequest)
    ) {
        return;
    }

    let Ok(probe_req) = packet.data.pread_with::<ProbeRequestFrame>(0, false) else {
        return;
    };

    let source = probe_req.header.transmitter_address;
    if !((source[0] == 84 && source[1] == 138 && source[2] == 186) // FOR TESTING PURPOSES: Filter out both CISCO and ESPRESSIF MAC-Addresses, to visualize "normal" devices
            || (source[0] == 52 && source[1] == 152 && source[2] == 122) || (source[0] == 112 && source[1] == 211 && source[2] == 121) || (source[0] == 16 && source[1] == 60 && source[2] == 89))
    {
        let elements = probe_req.body.get_elements();
        fingerprint_probe(elements.bytes);
    }
}
