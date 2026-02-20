use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Receiver;

use esp_radio::wifi::{PromiscuousPkt, Sniffer};
use log::{error, info};

#[derive(PartialEq)]
pub enum WifiCmd {
    StartSniffing,
    StopSniffing,
    EnableSta,
}

// pub enum WifiEvt { Currently unused
//     Online,
//     Sniffing,
// }

#[embassy_executor::task]
pub async fn wifi_manager_task(
    mut sniffer: Sniffer<'static>,
    callback: fn(PromiscuousPkt),
    receiver: Receiver<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    loop {
        let cmd = receiver.receive().await;
        if cmd == WifiCmd::StartSniffing {
            match sniffer.set_promiscuous_mode(true) {
                Ok(()) => {
                    info!("Enabled Promiscuous Mode");
                    sniffer.set_receive_cb(callback);
                }
                Err(e) => error!("Failed to enable promiscuous mode: {:?}", e),
            }
        } else if cmd == WifiCmd::StopSniffing {
            match sniffer.set_promiscuous_mode(false) {
                Ok(()) => info!("Disabled Promiscuous mode"),
                Err(e) => error!("Failed to disable promiscuous mode: {:?}", e),
            }
        }
    }
}
