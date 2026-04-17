use std::{
    env,
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use ieee80211::{
    common::{FrameControlField, FrameType, ManagementFrameSubtype},
    mgmt_frame::{body::HasElements, ProbeRequestFrame},
    scroll::Pread,
};

#[path = "../../../../trailsense-edge/src/probes/models.rs"]
mod models;

#[derive(Debug)]
struct FileResult {
    device: String,
    mode: String,
    path: PathBuf,
    probe_count: u32,
    dedup_count: u32,
}

#[derive(Debug)]
struct FileSample {
    device: String,
    mode: String,
    path: PathBuf,
    fingerprints: Vec<u64>,
}

#[derive(Default)]
struct Aggregate {
    files_total: u32,
    files_with_probes: u32,
    perfect_single_device: u32,
    zero_probe_files: u32,
    sum_abs_error: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    if !validate_model_config() {
        return Err("invalid model config (size/alpha/tau-range preconditions failed)".into());
    }

    let dataset_root = resolve_dataset_root()?;
    let args: Vec<String> = env::args().collect();
    let do_tau_sweep = args.iter().any(|a| a == "--sweep-tau");
    let tau_override = parse_tau_override(&args)?;

    let mut pcap_files = Vec::new();
    collect_pcap_files(&dataset_root, &mut pcap_files)?;
    pcap_files.sort();

    if pcap_files.is_empty() {
        return Err("no .pcap files found in dataset folder".into());
    }

    let mut samples = Vec::new();
    for file_path in pcap_files {
        let bytes = fs::read(&file_path)?;
        let fingerprints = extract_probe_fingerprints(&bytes)?;

        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid UTF-8 filename")?;
        let device = file_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        samples.push(FileSample {
            device,
            mode: parse_mode(file_name),
            path: fs::canonicalize(&file_path).unwrap_or(file_path),
            fingerprints,
        });
    }

    if do_tau_sweep {
        run_tau_sweep(&samples);
        return Ok(());
    }

    let eval_tau = tau_override.unwrap_or(models::TAU);
    if !eval_tau.is_finite() {
        return Err("tau must be finite".into());
    }
    let results = evaluate_at_tau(&samples, eval_tau);
    print_report(&results, eval_tau);
    Ok(())
}

fn parse_tau_override(args: &[String]) -> Result<Option<f32>, Box<dyn Error>> {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--tau" {
            let value = args.get(i + 1).ok_or("--tau requires a numeric value")?;
            let tau: f32 = value.parse()?;
            return Ok(Some(tau));
        }
        i += 1;
    }
    Ok(None)
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

fn parse_mode(file_name: &str) -> String {
    if let Some(start) = file_name.find("-mode") {
        let s = &file_name[(start + 5)..];
        if let Some(end) = s.find('-') {
            return s[..end].to_string();
        }
    }
    "?".to_string()
}

fn evaluate_at_tau(samples: &[FileSample], tau: f32) -> Vec<FileResult> {
    samples
        .iter()
        .map(|s| FileResult {
            device: s.device.clone(),
            mode: s.mode.clone(),
            path: s.path.clone(),
            probe_count: s.fingerprints.len() as u32,
            dedup_count: deduplicate_probes_tau(&s.fingerprints, tau),
        })
        .collect()
}

fn print_report(results: &[FileResult], tau: f32) {
    let mut overall = Aggregate::default();
    let mut per_device: BTreeMap<String, Aggregate> = BTreeMap::new();
    let mut per_mode: BTreeMap<String, Aggregate> = BTreeMap::new();

    for r in results {
        update_aggregate(&mut overall, r);
        update_aggregate(per_device.entry(r.device.clone()).or_default(), r);
        update_aggregate(per_mode.entry(r.mode.clone()).or_default(), r);
    }

    println!("=== TrailSense single-device evaluation on Dataset #2 ===");
    println!(
        "Model: {} (size={}, tau={:.4})",
        models::DEDUP_MODEL_VERSION,
        models::MODEL_SIZE,
        tau
    );
    println!("Files scanned: {}", overall.files_total);
    println!("Files with zero accepted probe requests: {}", overall.zero_probe_files);
    println!("Files with accepted probes: {}", overall.files_with_probes);
    println!(
        "Single-device accuracy (dedup_count == 1): {:.2}%",
        percent(overall.perfect_single_device, overall.files_with_probes)
    );
    println!(
        "Mean absolute count error vs expected=1: {:.4}",
        mean_abs_error(&overall)
    );

    println!("\n-- Per device --");
    println!("device\tfiles\twith_probes\tperfect\taccuracy\tmae");
    for (device, agg) in &per_device {
        println!(
            "{}\t{}\t{}\t{}\t{:.2}%\t{:.4}",
            device,
            agg.files_total,
            agg.files_with_probes,
            agg.perfect_single_device,
            percent(agg.perfect_single_device, agg.files_with_probes),
            mean_abs_error(agg)
        );
    }

    println!("\n-- Per mode --");
    println!("mode\tfiles\twith_probes\tperfect\taccuracy\tmae");
    for (mode, agg) in &per_mode {
        println!(
            "{}\t{}\t{}\t{}\t{:.2}%\t{:.4}",
            mode,
            agg.files_total,
            agg.files_with_probes,
            agg.perfect_single_device,
            percent(agg.perfect_single_device, agg.files_with_probes),
            mean_abs_error(agg)
        );
    }

    let mut worst: Vec<&FileResult> = results
        .iter()
        .filter(|r| r.probe_count > 0 && r.dedup_count != 1)
        .collect();
    worst.sort_by_key(|r| std::cmp::Reverse(r.dedup_count));

    println!("\n-- Worst files (top 12 by dedup_count) --");
    println!("device\tmode\tdedup\tprobes\tfile");
    for r in worst.into_iter().take(12) {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            r.device,
            r.mode,
            r.dedup_count,
            r.probe_count,
            r.path.to_string_lossy()
        );
    }

    let zero_probe: Vec<&FileResult> = results.iter().filter(|r| r.probe_count == 0).collect();
    if !zero_probe.is_empty() {
        println!("\n-- Zero-probe files --");
        println!("device\tmode\tfile");
        for r in zero_probe.into_iter().take(20) {
            println!("{}\t{}\t{}", r.device, r.mode, r.path.to_string_lossy());
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SweepPoint {
    tau: f32,
    with_probes: u32,
    perfect: u32,
    mae: f64,
}

fn run_tau_sweep(samples: &[FileSample]) {
    let alpha_sum = model_alpha_sum();
    let current_tau = models::TAU;

    let coarse_min = f32::max(-alpha_sum, current_tau - 30.0);
    let coarse_max = f32::min(alpha_sum, current_tau + 30.0);
    let coarse = sweep_range(samples, coarse_min, coarse_max, 1.0);
    let coarse_best = coarse
        .iter()
        .copied()
        .max_by(sweep_cmp)
        .expect("coarse sweep must not be empty");

    let fine_min = f32::max(-alpha_sum, coarse_best.tau - 2.0);
    let fine_max = f32::min(alpha_sum, coarse_best.tau + 2.0);
    let mut fine = sweep_range(samples, fine_min, fine_max, 0.1);
    fine.push(coarse_best);

    fine.sort_by(sweep_cmp);
    fine.reverse();

    let baseline = evaluate_point(samples, current_tau);
    let best = fine[0];

    println!("=== TAU sweep on Dataset #2 ===");
    println!(
        "Model: {} (size={}, current_tau={:.4}, alpha_sum={:.4})",
        models::DEDUP_MODEL_VERSION,
        models::MODEL_SIZE,
        current_tau,
        alpha_sum
    );
    println!(
        "Baseline: tau={:.4} | accuracy={:.2}% ({}/{}) | mae={:.4}",
        baseline.tau,
        percent(baseline.perfect, baseline.with_probes),
        baseline.perfect,
        baseline.with_probes,
        baseline.mae
    );
    println!(
        "Best:     tau={:.4} | accuracy={:.2}% ({}/{}) | mae={:.4}",
        best.tau,
        percent(best.perfect, best.with_probes),
        best.perfect,
        best.with_probes,
        best.mae
    );
    println!(
        "Delta:    accuracy={:+.2} pts | mae={:+.4}",
        percent(best.perfect, best.with_probes) - percent(baseline.perfect, baseline.with_probes),
        best.mae - baseline.mae
    );

    println!("\n-- Top 10 TAU values --");
    println!("rank\ttau\taccuracy\tperfect/with_probes\tmae");
    for (idx, p) in fine.iter().take(10).enumerate() {
        println!(
            "{}\t{:.4}\t{:.2}%\t{}/{}\t{:.4}",
            idx + 1,
            p.tau,
            percent(p.perfect, p.with_probes),
            p.perfect,
            p.with_probes,
            p.mae
        );
    }
}

fn sweep_range(samples: &[FileSample], tau_min: f32, tau_max: f32, step: f32) -> Vec<SweepPoint> {
    let mut out = Vec::new();
    let mut tau = tau_min;
    while tau <= tau_max + 1e-6 {
        out.push(evaluate_point(samples, tau));
        tau += step;
    }
    out
}

fn evaluate_point(samples: &[FileSample], tau: f32) -> SweepPoint {
    let mut agg = Aggregate::default();

    for s in samples {
        agg.files_total += 1;
        let probe_count = s.fingerprints.len() as u32;
        if probe_count == 0 {
            agg.zero_probe_files += 1;
            continue;
        }

        let dedup_count = deduplicate_probes_tau(&s.fingerprints, tau);
        agg.files_with_probes += 1;
        if dedup_count == 1 {
            agg.perfect_single_device += 1;
        }
        agg.sum_abs_error += dedup_count.abs_diff(1) as u64;
    }

    SweepPoint {
        tau,
        with_probes: agg.files_with_probes,
        perfect: agg.perfect_single_device,
        mae: mean_abs_error(&agg),
    }
}

fn sweep_cmp(a: &SweepPoint, b: &SweepPoint) -> std::cmp::Ordering {
    a.perfect
        .cmp(&b.perfect)
        .then_with(|| b.mae.partial_cmp(&a.mae).unwrap_or(std::cmp::Ordering::Equal))
}

fn update_aggregate(agg: &mut Aggregate, r: &FileResult) {
    agg.files_total += 1;
    if r.probe_count == 0 {
        agg.zero_probe_files += 1;
        return;
    }

    agg.files_with_probes += 1;
    if r.dedup_count == 1 {
        agg.perfect_single_device += 1;
    }

    agg.sum_abs_error += r.dedup_count.abs_diff(1) as u64;
}

fn percent(num: u32, den: u32) -> f64 {
    if den == 0 {
        return 0.0;
    }
    (num as f64 * 100.0) / den as f64
}

fn mean_abs_error(agg: &Aggregate) -> f64 {
    if agg.files_with_probes == 0 {
        return 0.0;
    }
    agg.sum_abs_error as f64 / agg.files_with_probes as f64
}

fn extract_probe_fingerprints(pcap: &[u8]) -> Result<Vec<u64>, Box<dyn Error>> {
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

        if let Some(fp) = packet_to_fingerprint(packet) {
            out.push(fp);
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

fn packet_to_fingerprint(packet: &[u8]) -> Option<u64> {
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
    Some(fingerprint_probe(elements.bytes))
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

fn deduplicate_probes_tau(input_fingerprints: &[u64], tau: f32) -> u32 {
    if input_fingerprints.is_empty() {
        return 0;
    }

    let mut survivors: Vec<u64> = Vec::new();
    survivors.push(input_fingerprints[0]);

    for &fingerprint in &input_fingerprints[1..] {
        if !is_duplicate(fingerprint, &survivors, tau) {
            survivors.push(fingerprint);
        }
    }

    survivors.len() as u32
}

fn is_duplicate(input: u64, survivors: &[u64], tau: f32) -> bool {
    survivors.iter().any(|&s| weighted_score(input, s) >= tau)
}

fn weighted_score(a: u64, b: u64) -> f32 {
    let mut score = 0.0_f32;

    for i in 0..models::MODEL_SIZE {
        let bit_pos = models::MODEL_SIZE - 1 - i;
        let mask = 1u64 << bit_pos;

        if (a & mask) == (b & mask) {
            score += models::MODEL[i].alpha;
        } else {
            score -= models::MODEL[i].alpha;
        }
    }

    score
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
