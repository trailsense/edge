use embassy_time::{Duration, Timer};
use esp_radio::wifi::{PromiscuousPkt, Sniffer};
use log::info;

#[embassy_executor::task]
pub async fn wifi_manager_task(mut sniffer: Sniffer<'static>, callback: fn(PromiscuousPkt)) {
    sniffer
        .set_promiscuous_mode(true)
        .expect("Failed to enable promiscuous mode");
    sniffer.set_receive_cb(callback);

    info!("Sniffer enabled, callback installed");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
