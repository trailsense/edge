pub const RECOVERY_STAGE_COUNT: u8 = 3;

pub fn connect_recovery_stage_index(streak: u8) -> u8 {
    streak.saturating_sub(1) % RECOVERY_STAGE_COUNT
}

pub fn upload_recovery_stage_index(streak: u8, threshold: u8) -> Option<u8> {
    if streak < threshold {
        None
    } else {
        Some((streak - threshold) % RECOVERY_STAGE_COUNT)
    }
}
