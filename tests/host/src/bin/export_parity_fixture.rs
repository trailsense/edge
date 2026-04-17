use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use ieee80211::{
    common::{FrameControlField, FrameType, ManagementFrameSubtype},
    mgmt_frame::{body::HasElements, ProbeRequestFrame},
    scroll::Pread,
};
use serde::Serialize;

#[path = "../../../../trailsense-edge/src/probes/models.rs"]
mod models;

#[derive(Serialize)]
struct ParityFixture {
    model_version: String,
    model_size: usize,
    tau: f32,
    classifiers: Vec<ClassifierExport>,
    samples: Vec<ProbeSample>,
}

#[derive(Serialize)]
struct ClassifierExport {
    positive_mask: Vec<u8>,
    negative_mask: Vec<u8>,
    threshold: u32,
    alpha: f32,
}

#[derive(Serialize)]
struct ProbeSample {
    device: String,
    file: String,
    probe_index_in_file: usize,
    elements_bytes: Vec<u8>,
    rust_fingerprint: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    if !validate_model_config() {
        return Err("invalid model config (size/alpha/tau-range preconditions failed)".into());
    }

    let args: Vec<String> = env::args().collect();
    let out_path = parse_out_path(&args).unwrap_or_else(|| PathBuf::from("tests/host/parity_fixture.json"));
    let max_samples = parse_max_samples(&args).unwrap_or(500);

    let dataset_root = resolve_dataset_root()?;
    let mut pcap_files = Vec::new();
    collect_pcap_files(&dataset_root, &mut pcap_files)?;
    pcap_files.sort();

    if pcap_files.is_empty() {
        return Err("no .pcap files found in dataset folder".into());
    }

    let mut samples: Vec<ProbeSample> = Vec::new();

    'outer: for pcap_path in pcap_files {
        let file_bytes = fs::read(&pcap_path)?;
        let probe_elements = extract_probe_elements(&file_bytes)?;

        let file_name = pcap_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid UTF-8 filename")?
            .to_string();
        let device = pcap_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        for (idx, elements_bytes) in probe_elements.into_iter().enumerate() {
            let rust_fingerprint = fingerprint_probe(&elements_bytes);
            samples.push(ProbeSample {
                device: device.clone(),
                file: file_name.clone(),
                probe_index_in_file: idx,
                elements_bytes,
                rust_fingerprint,
            });
            if samples.len() >= max_samples {
                break 'outer;
            }
        }
    }

    if samples.is_empty() {
        return Err("no probe-request samples extracted (all filtered or parse failed)".into());
    }

    let classifiers = models::MODEL
        .iter()
        .map(|m| ClassifierExport {
            positive_mask: m.positive_mask.to_vec(),
            negative_mask: m.negative_mask.to_vec(),
            threshold: m.threshold,
            alpha: m.alpha,
        })
        .collect::<Vec<_>>();

    let fixture = ParityFixture {
        model_version: models::DEDUP_MODEL_VERSION.to_string(),
        model_size: models::MODEL_SIZE,
        tau: models::TAU,
        classifiers,
        samples,
    };

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(&out_path, serde_json::to_vec_pretty(&fixture)?)?;

    println!(
        "Wrote parity fixture to {} (samples={})",
        out_path.to_string_lossy(),
        fixture.samples.len()
    );
    Ok(())
}

fn parse_out_path(args: &[String]) -> Option<PathBuf> {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--out" {
            return args.get(i + 1).map(PathBuf::from);
        }
        i += 1;
    }
    None
}

fn parse_max_samples(args: &[String]) -> Option<usize> {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--max-samples" {
            return args.get(i + 1).and_then(|v| v.parse::<usize>().ok());
        }
        i += 1;
    }
    None
}

fn resolve_dataset_root() -> Result<PathBuf, Box<dyn Error>> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let candidates = [
        manifest_dir.join("../training_data/Individual devices"),
        manifest_dir.join("../A dataset of labelled device Wi-Fi probe requests/Individual devices"),
    ];

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err("dataset folder not found in expected locations under tests/".into())
}

fn collect_pcap_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_pcap_files(&path, out)?;
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pcap"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn extract_probe_elements(pcap: &[u8]) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    if pcap.len() < 24 {
        return Err("pcap too short for global header".into());
    }

    let magic = [pcap[0], pcap[1], pcap[2], pcap[3]];
    let little_endian = match magic {
        [0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1] => true,
        [0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d] => false,
        _ => {
            return Err(format!(
                "unsupported pcap magic: {:02x}{:02x}{:02x}{:02x}",
                magic[0], magic[1], magic[2], magic[3]
            )
            .into())
        }
    };

    let mut offset = 24usize;
    let mut out = Vec::new();

    while offset + 16 <= pcap.len() {
        let incl_len = read_u32(&pcap[(offset + 8)..(offset + 12)], little_endian)? as usize;
        offset += 16;

        if offset + incl_len > pcap.len() {
            break;
        }

        let packet = &pcap[offset..(offset + incl_len)];
        offset += incl_len;

        if let Some(elements_bytes) = packet_to_probe_elements(packet) {
            out.push(elements_bytes);
        }
    }

    Ok(out)
}

fn read_u32(bytes: &[u8], little_endian: bool) -> Result<u32, Box<dyn Error>> {
    if bytes.len() != 4 {
        return Err("read_u32 expects exactly 4 bytes".into());
    }
    Ok(if little_endian {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    })
}

fn packet_to_probe_elements(packet: &[u8]) -> Option<Vec<u8>> {
    if packet.len() < 4 {
        return None;
    }

    let radiotap_len = u16::from_le_bytes([packet[2], packet[3]]) as usize;
    if radiotap_len >= packet.len() || packet.len() - radiotap_len < 2 {
        return None;
    }

    let frame = &packet[radiotap_len..];
    let fcf = FrameControlField::from_bits(u16::from_le_bytes([frame[0], frame[1]]));
    if !matches!(
        fcf.frame_type(),
        FrameType::Management(ManagementFrameSubtype::ProbeRequest)
    ) {
        return None;
    }

    let probe_req = frame.pread_with::<ProbeRequestFrame>(0, false).ok()?;
    let source = probe_req.header.transmitter_address;

    if (source[0] == 84 && source[1] == 138 && source[2] == 186)
        || (source[0] == 52 && source[1] == 152 && source[2] == 122)
        || (source[0] == 112 && source[1] == 211 && source[2] == 121)
        || (source[0] == 16 && source[1] == 60 && source[2] == 89)
    {
        return None;
    }

    let elements = probe_req.body.get_elements();
    Some(elements.bytes.to_vec())
}

fn fingerprint_probe(data: &[u8]) -> u64 {
    let mut fingerprint: u64 = 0;
    debug_assert_eq!(models::MODEL_SIZE, 64);

    for model in models::MODEL {
        let max_iterations = core::cmp::min(
            data.len(),
            core::cmp::min(model.positive_mask.len(), model.negative_mask.len()),
        );
        let mut score: i32 = 0;
        for i in 0..max_iterations {
            let positive_bits = data[i] & model.positive_mask[i];
            let negative_bits = data[i] & model.negative_mask[i];
            score += positive_bits.count_ones() as i32;
            score -= negative_bits.count_ones() as i32;
        }
        let bit = if score > model.threshold as i32 { 1 } else { 0 };
        fingerprint = (fingerprint << 1) | bit;
    }

    fingerprint
}

fn validate_model_config() -> bool {
    if models::MODEL_SIZE == 0 || models::MODEL_SIZE > 64 {
        return false;
    }

    let alpha_sum = model_alpha_sum();
    alpha_sum.is_finite() && models::TAU.is_finite() && models::TAU >= -alpha_sum && models::TAU <= alpha_sum
}

fn model_alpha_sum() -> f32 {
    let mut alpha_sum = 0.0_f32;
    for model in models::MODEL {
        if !model.alpha.is_finite() || model.alpha <= 0.0 {
            return f32::NAN;
        }
        alpha_sum += model.alpha;
    }
    alpha_sum
}
