pub const NETWORK_LIMIT: u8 = 5;
pub const MAX_LOCAL_SAVES: u8 = 10;
pub const MAX_SAVE_FAILURES: u8 = 5;

pub fn bump_counter(value: u8) -> u8 {
    value.saturating_add(1)
}

pub fn should_fallback_to_saving(network_error_count: u8) -> bool {
    network_error_count >= NETWORK_LIMIT
}

pub fn should_enter_sleep(network_fallback_count: u8, save_failure_count: u8) -> bool {
    network_fallback_count >= MAX_LOCAL_SAVES || save_failure_count >= MAX_SAVE_FAILURES
}
