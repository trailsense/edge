use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::{Flex, InputConfig, OutputConfig};
use log::info;

use crate::hardware::types::HardwareCmd;

#[cfg(feature = "uplink-gsm")]
const FORCE_POWER_DOWN_ASSERT_SECS: u64 = 16;
#[cfg(feature = "uplink-gsm")]
const POWER_DOWN_SETTLE_SECS: u64 = 2;
#[cfg(feature = "uplink-gsm")]
const POWER_ON_TAP_SECS: u64 = 2;
#[cfg(feature = "uplink-gsm")]
const POWER_ON_SETTLE_SECS: u64 = 5;

#[cfg(feature = "uplink-gsm")]
fn drive_pwrkey_low(pin: &mut Flex<'_>) {
    pin.apply_output_config(&OutputConfig::default());
    pin.set_input_enable(false);
    pin.set_low();
    pin.set_output_enable(true);
}

#[cfg(feature = "uplink-gsm")]
fn release_pwrkey_high_z(pin: &mut Flex<'_>) {
    pin.set_output_enable(false);
    pin.apply_input_config(&InputConfig::default());
    pin.set_input_enable(true);
}

// This is test code for power reset. Does not work for our breakout model. But it could work if we build our custom pcb. It is not included in build
#[cfg(feature = "uplink-gsm")]
pub async fn pulse_gsm_reset_pin(gsm_reset_pin: &mut Flex<'_>) {
    // A7670E force reboot via PWRKEY:
    // 1) Hold LOW long enough to force power down
    // 2) Release to high-Z and wait for full power-down
    // 3) Hold LOW briefly to power back on
    // 4) Release to high-Z and wait for boot
    info!(
        "GSM reset: force power down (hold LOW {}s)",
        FORCE_POWER_DOWN_ASSERT_SECS
    );
    drive_pwrkey_low(gsm_reset_pin);
    Timer::after(Duration::from_secs(FORCE_POWER_DOWN_ASSERT_SECS)).await;

    info!(
        "GSM reset: release PWRKEY, settling {}s",
        POWER_DOWN_SETTLE_SECS
    );
    release_pwrkey_high_z(gsm_reset_pin);
    Timer::after(Duration::from_secs(POWER_DOWN_SETTLE_SECS)).await;

    info!("GSM reset: power on tap (hold LOW {}s)", POWER_ON_TAP_SECS);
    drive_pwrkey_low(gsm_reset_pin);
    Timer::after(Duration::from_secs(POWER_ON_TAP_SECS)).await;

    release_pwrkey_high_z(gsm_reset_pin);
    info!(
        "GSM reset: sequence complete, boot settle {}s",
        POWER_ON_SETTLE_SECS
    );
    Timer::after(Duration::from_secs(POWER_ON_SETTLE_SECS)).await;
}

#[cfg(feature = "uplink-gsm")]
#[embassy_executor::task]
pub async fn trigger_gsm_reset(
    mut gsm_reset_pin: Flex<'static>,
    hardware_receiver: Receiver<'static, CriticalSectionRawMutex, HardwareCmd, 4>,
) -> ! {
    loop {
        let command = hardware_receiver.receive().await;
        match command {
            HardwareCmd::ResetGSM => {
                pulse_gsm_reset_pin(&mut gsm_reset_pin).await;
            }
        }
    }
}
