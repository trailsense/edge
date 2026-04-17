use std::{
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

const MAX_SINGLE_PROBES: usize = 96;
const MAX_MIX_PROBES_PER_DEVICE: usize = 24;
const MAX_TRIPLES_PER_GROUP: usize = 120;
const IMBALANCE_WEIGHT: f64 = 0.25;

#[derive(Debug, Clone)]
struct FileSample {
    device: String,
    mode: String,
    channel: u8,
    fingerprints: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WindowKind {
    Single,
    Pair,
    Triple,
}

#[derive(Debug, Clone)]
struct Window {
    channel: u8,
    kind: WindowKind,
    expected_count: u32,
    fingerprints: Vec<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Metrics {
    windows: u32,
    exact: u32,
    expected_sum: u64,
    pred_sum: u64,
    abs_err_sum: u64,
    under_sum: u64,
    over_sum: u64,
}

#[derive(Debug, Clone, Copy)]
struct SweepPoint {
    tau: f32,
    objective: f64,
    metrics_all: Metrics,
}

fn main() -> Result<(), Box<dyn Error>> {
    if !validate_model_config() {
        return Err("invalid model config (size/alpha/tau-range preconditions failed)".into());
    }

    let dataset_root = resolve_dataset_root()?;
    let mut pcap_files = Vec::new();
    collect_pcap_files(&dataset_root, &mut pcap_files)?;
    pcap_files.sort();

    if pcap_files.is_empty() {
        return Err("no .pcap files found in dataset folder".into());
    }

    let mut samples = Vec::new();
    for file_path in pcap_files {
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
        let mode = parse_mode(file_name);
        let Some(channel) = parse_channel(file_name) else {
            continue;
        };

        let bytes = fs::read(&file_path)?;
        let fingerprints = extract_probe_fingerprints(&bytes)?;
        if fingerprints.is_empty() {
            continue;
        }

        samples.push(FileSample {
            device,
            mode,
            channel,
            fingerprints,
        });
    }

    if samples.is_empty() {
        return Err("no probe fingerprints extracted from dataset".into());
    }

    let windows = build_windows(&samples);
    if windows.is_empty() {
        return Err("no evaluation windows built from dataset".into());
    }

    let channels = collect_channels(&windows);
    if channels.is_empty() {
        return Err("no channels found in evaluation windows".into());
    }

    let alpha_sum = model_alpha_sum();
    let current_tau = models::TAU;

    let coarse = sweep_range(&windows, &channels, -alpha_sum, alpha_sum, 1.0);
    let coarse_best = *coarse
        .iter()
        .min_by(|a, b| compare_points(a, b, current_tau))
        .ok_or("empty coarse sweep")?;

    let fine_min = (coarse_best.tau - 2.5).max(-alpha_sum);
    let fine_max = (coarse_best.tau + 2.5).min(alpha_sum);
    let mut fine = sweep_range(&windows, &channels, fine_min, fine_max, 0.1);
    fine.push(coarse_best);

    fine.sort_by(|a, b| compare_points(a, b, current_tau));

    let baseline = evaluate_tau(&windows, &channels, current_tau);
    let best = fine[0];

    println!("=== Production-oriented TAU optimization ===");
    println!(
        "Model: {} | bits={} | current_tau={:.4} | alpha_sum={:.4}",
        models::DEDUP_MODEL_VERSION,
        models::MODEL_SIZE,
        current_tau,
        alpha_sum
    );
    println!(
        "Dataset files with probes: {} | windows: {} (single/pair/triple = {}/{}/{})",
        samples.len(),
        windows.len(),
        windows.iter().filter(|w| w.kind == WindowKind::Single).count(),
        windows.iter().filter(|w| w.kind == WindowKind::Pair).count(),
        windows.iter().filter(|w| w.kind == WindowKind::Triple).count()
    );
    println!(
        "Channels used for holdout folds: {}",
        channels
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();

    print_point("Baseline", baseline);
    print_point("Best", best);
    println!(
        "Delta: objective={:+.6}, exact={:+.2} pts, mae={:+.4}, under_rate={:+.4}, over_rate={:+.4}",
        best.objective - baseline.objective,
        metrics_exact_accuracy(best.metrics_all) - metrics_exact_accuracy(baseline.metrics_all),
        metrics_mae(best.metrics_all) - metrics_mae(baseline.metrics_all),
        metrics_under_rate(best.metrics_all) - metrics_under_rate(baseline.metrics_all),
        metrics_over_rate(best.metrics_all) - metrics_over_rate(baseline.metrics_all),
    );
    println!();

    println!("-- Top 10 tau candidates --");
    println!("rank\ttau\tobjective\texact\tmae\tunder_rate\tover_rate");
    for (idx, p) in fine.iter().take(10).enumerate() {
        println!(
            "{}\t{:.4}\t{:.6}\t{:.2}%\t{:.4}\t{:.4}\t{:.4}",
            idx + 1,
            p.tau,
            p.objective,
            metrics_exact_accuracy(p.metrics_all),
            metrics_mae(p.metrics_all),
            metrics_under_rate(p.metrics_all),
            metrics_over_rate(p.metrics_all)
        );
    }
    println!();

    println!("-- Best tau breakdown by window kind --");
    for kind in [WindowKind::Single, WindowKind::Pair, WindowKind::Triple] {
        let metrics = evaluate_tau_kind(&windows, best.tau, kind);
        println!(
            "{:?}: windows={}, exact={:.2}%, mae={:.4}, under_rate={:.4}, over_rate={:.4}",
            kind,
            metrics.windows,
            metrics_exact_accuracy(metrics),
            metrics_mae(metrics),
            metrics_under_rate(metrics),
            metrics_over_rate(metrics)
        );
    }

    Ok(())
}

fn print_point(label: &str, point: SweepPoint) {
    println!(
        "{}: tau={:.4} | objective={:.6} | exact={:.2}% | mae={:.4} | under_rate={:.4} | over_rate={:.4}",
        label,
        point.tau,
        point.objective,
        metrics_exact_accuracy(point.metrics_all),
        metrics_mae(point.metrics_all),
        metrics_under_rate(point.metrics_all),
        metrics_over_rate(point.metrics_all),
    );
}

fn compare_points(a: &SweepPoint, b: &SweepPoint, current_tau: f32) -> std::cmp::Ordering {
    a.objective
        .partial_cmp(&b.objective)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            metrics_exact_accuracy(b.metrics_all)
                .partial_cmp(&metrics_exact_accuracy(a.metrics_all))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| {
            (a.tau - current_tau)
                .abs()
                .partial_cmp(&(b.tau - current_tau).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn sweep_range(
    windows: &[Window],
    channels: &[u8],
    tau_min: f32,
    tau_max: f32,
    step: f32,
) -> Vec<SweepPoint> {
    let mut out = Vec::new();
    let mut tau = tau_min;
    while tau <= tau_max + 1e-6 {
        out.push(evaluate_tau(windows, channels, tau));
        tau += step;
    }
    out
}

fn evaluate_tau(windows: &[Window], channels: &[u8], tau: f32) -> SweepPoint {
    let mut all = Metrics::default();
    let mut objective_sum = 0.0_f64;
    let mut folds = 0_u32;

    for &channel in channels {
        let mut fold = Metrics::default();
        for w in windows.iter().filter(|w| w.channel == channel) {
            let pred = deduplicate_probes_tau(&w.fingerprints, tau);
            fold = update_metrics(fold, w.expected_count, pred);
        }

        if fold.windows > 0 {
            objective_sum += fold_objective(fold);
            folds += 1;
        }
        all = merge_metrics(all, fold);
    }

    let objective = if folds == 0 {
        f64::INFINITY
    } else {
        objective_sum / folds as f64
    };

    SweepPoint {
        tau,
        objective,
        metrics_all: all,
    }
}

fn evaluate_tau_kind(windows: &[Window], tau: f32, kind: WindowKind) -> Metrics {
    let mut metrics = Metrics::default();
    for w in windows.iter().filter(|w| w.kind == kind) {
        let pred = deduplicate_probes_tau(&w.fingerprints, tau);
        metrics = update_metrics(metrics, w.expected_count, pred);
    }
    metrics
}

fn fold_objective(metrics: Metrics) -> f64 {
    let mae = metrics_mae(metrics);
    let imbalance = (metrics_under_rate(metrics) - metrics_over_rate(metrics)).abs();
    mae + IMBALANCE_WEIGHT * imbalance
}

fn update_metrics(mut m: Metrics, expected: u32, predicted: u32) -> Metrics {
    m.windows += 1;
    m.expected_sum += expected as u64;
    m.pred_sum += predicted as u64;
    if expected == predicted {
        m.exact += 1;
    }

    if predicted >= expected {
        m.over_sum += (predicted - expected) as u64;
    } else {
        m.under_sum += (expected - predicted) as u64;
    }
    m.abs_err_sum += predicted.abs_diff(expected) as u64;
    m
}

fn merge_metrics(a: Metrics, b: Metrics) -> Metrics {
    Metrics {
        windows: a.windows + b.windows,
        exact: a.exact + b.exact,
        expected_sum: a.expected_sum + b.expected_sum,
        pred_sum: a.pred_sum + b.pred_sum,
        abs_err_sum: a.abs_err_sum + b.abs_err_sum,
        under_sum: a.under_sum + b.under_sum,
        over_sum: a.over_sum + b.over_sum,
    }
}

fn metrics_exact_accuracy(m: Metrics) -> f64 {
    if m.windows == 0 {
        return 0.0;
    }
    (m.exact as f64) * 100.0 / (m.windows as f64)
}

fn metrics_mae(m: Metrics) -> f64 {
    if m.windows == 0 {
        return 0.0;
    }
    m.abs_err_sum as f64 / m.windows as f64
}

fn metrics_under_rate(m: Metrics) -> f64 {
    if m.expected_sum == 0 {
        return 0.0;
    }
    m.under_sum as f64 / m.expected_sum as f64
}

fn metrics_over_rate(m: Metrics) -> f64 {
    if m.expected_sum == 0 {
        return 0.0;
    }
    m.over_sum as f64 / m.expected_sum as f64
}

fn collect_channels(windows: &[Window]) -> Vec<u8> {
    let mut channels = windows.iter().map(|w| w.channel).collect::<Vec<_>>();
    channels.sort();
    channels.dedup();
    channels
}

fn build_windows(samples: &[FileSample]) -> Vec<Window> {
    let mut windows = Vec::new();

    // 1-device windows
    for s in samples {
        windows.push(Window {
            channel: s.channel,
            kind: WindowKind::Single,
            expected_count: 1,
            fingerprints: s
                .fingerprints
                .iter()
                .copied()
                .take(MAX_SINGLE_PROBES)
                .collect(),
        });
    }

    // Group by (mode, channel) to avoid trivial leakage from mixed channels in a window.
    let mut groups: BTreeMap<(String, u8), Vec<&FileSample>> = BTreeMap::new();
    for s in samples {
        groups.entry((s.mode.clone(), s.channel)).or_default().push(s);
    }

    for ((_mode, channel), mut group) in groups {
        group.sort_by(|a, b| a.device.cmp(&b.device));
        if group.len() < 2 {
            continue;
        }

        // 2-device windows: all pair combinations.
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                if group[i].device == group[j].device {
                    continue;
                }
                windows.push(Window {
                    channel,
                    kind: WindowKind::Pair,
                    expected_count: 2,
                    fingerprints: interleave_fingerprints(&[
                        &group[i].fingerprints,
                        &group[j].fingerprints,
                    ]),
                });
            }
        }

        // 3-device windows: sampled evenly if there are too many.
        if group.len() >= 3 {
            let total = n_choose_k(group.len(), 3);
            let selected_positions = sampled_positions(total, MAX_TRIPLES_PER_GROUP);
            let mut next_pos_idx = 0usize;
            let mut combo_pos = 0usize;

            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    for k in (j + 1)..group.len() {
                        if next_pos_idx < selected_positions.len()
                            && combo_pos == selected_positions[next_pos_idx]
                        {
                            windows.push(Window {
                                channel,
                                kind: WindowKind::Triple,
                                expected_count: 3,
                                fingerprints: interleave_fingerprints(&[
                                    &group[i].fingerprints,
                                    &group[j].fingerprints,
                                    &group[k].fingerprints,
                                ]),
                            });
                            next_pos_idx += 1;
                        }
                        combo_pos += 1;
                    }
                }
            }
        }
    }

    windows
}

fn interleave_fingerprints(parts: &[&[u64]]) -> Vec<u64> {
    let capped = parts
        .iter()
        .map(|p| p.iter().copied().take(MAX_MIX_PROBES_PER_DEVICE).collect::<Vec<_>>())
        .collect::<Vec<_>>();

    let max_len = capped.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::new();
    for idx in 0..max_len {
        for p in &capped {
            if idx < p.len() {
                out.push(p[idx]);
            }
        }
    }
    out
}

fn n_choose_k(n: usize, k: usize) -> usize {
    if k > n {
        return 0;
    }
    if k == 0 || k == n {
        return 1;
    }
    if k == 1 {
        return n;
    }
    if k == 2 {
        return n * (n - 1) / 2;
    }
    if k == 3 {
        return n * (n - 1) * (n - 2) / 6;
    }
    // not needed here
    0
}

fn sampled_positions(total: usize, max_keep: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    if total <= max_keep || max_keep <= 1 {
        return (0..total).collect();
    }

    let mut out = Vec::with_capacity(max_keep);
    for i in 0..max_keep {
        let pos = i * (total - 1) / (max_keep - 1);
        out.push(pos);
    }
    out.dedup();
    out
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

fn parse_channel(file_name: &str) -> Option<u8> {
    let start = file_name.find("-ch-")?;
    let s = &file_name[(start + 4)..];
    let end = s.find('-')?;
    s[..end].parse::<u8>().ok()
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
