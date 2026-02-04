use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Receiver;

use esp_radio::wifi::{PromiscuousPkt, Sniffer};
use log::info;

use crate::wifi::{http, wait_for_connection};

#[derive(PartialEq)]
pub enum WifiCmd {
    StartSniffing,
    StopSniffing,
    EnableSta,
}

pub enum WifiEvt {
    Online,
    Sniffing,
}

#[embassy_executor::task]
pub async fn wifi_manager_task(
    mut sniffer: Sniffer<'static>,
    callback: fn(PromiscuousPkt),
    receiver: Receiver<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    sniffer
        .set_promiscuous_mode(true)
        .expect("Failed to enable promiscuous mode");
    // sniffer.set_receive_cb(callback);
    info!("Sniffer enabled, callback installed");
    loop {
        let cmd = receiver.receive().await;
        if cmd == WifiCmd::StartSniffing {
            sniffer
                .set_promiscuous_mode(true)
                .expect("Failed to enable promiscuous mode");
            info!("Enabled Promiscuous Mode")
            // sniffer.set_receive_cb(callback);
        } else if cmd == WifiCmd::StopSniffing {
            sniffer
                .set_promiscuous_mode(false)
                .expect("Failed to disable promiscuous mode");
            info!("Disabled Promiscuous mode")
        }
    }
}
