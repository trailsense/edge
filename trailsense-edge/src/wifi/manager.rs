use embassy_sync::channel::Receiver;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, pubsub::Publisher};

use embassy_time::{Duration, Timer};
use esp_radio::wifi::{PromiscuousPkt, Sniffer};
use log::{error, info};

use crate::orchestration::types::{SnifferEvents, SystemCmd, SystemEvents};

const RADIO_SETTLE_DELAY: Duration = Duration::from_secs(5);

#[embassy_executor::task]
pub async fn wifi_manager_task(
    mut sniffer: Sniffer<'static>,
    callback: fn(PromiscuousPkt),
    sniffer_command_receiver: Receiver<'static, CriticalSectionRawMutex, SystemCmd, 4>,
    orchestrator_event_publisher: Publisher<
        'static,
        CriticalSectionRawMutex,
        SystemEvents,
        4,
        1,
        3,
    >,
) {
    loop {
        let command = sniffer_command_receiver.receive().await;

        match command {
            SystemCmd::StartSniffing { id } => match sniffer.set_promiscuous_mode(true) {
                Ok(()) => {
                    info!("Enabled Promiscuous Mode");
                    sniffer.set_receive_cb(callback);
                    orchestrator_event_publisher
                        .publish(SystemEvents::Sniffer {
                            id,
                            event: SnifferEvents::StartedSniffing,
                        })
                        .await;
                }
                Err(e) => {
                    error!("Failed to enable promiscuous mode: {:?}", e);
                    orchestrator_event_publisher
                        .publish(SystemEvents::Sniffer {
                            id,
                            event: SnifferEvents::SniffingError,
                        })
                        .await;
                }
            },

            SystemCmd::StopSniffing { id } => match sniffer.set_promiscuous_mode(false) {
                Ok(()) => {
                    info!("Disabled Promiscuous mode");
                    Timer::after(RADIO_SETTLE_DELAY).await;
                    orchestrator_event_publisher
                        .publish(SystemEvents::Sniffer {
                            id,
                            event: SnifferEvents::StoppedSniffing,
                        })
                        .await;
                }
                Err(e) => {
                    error!("Failed to disable promiscuous mode: {:?}", e);
                    orchestrator_event_publisher
                        .publish(SystemEvents::Sniffer {
                            id,
                            event: SnifferEvents::SniffingError,
                        })
                        .await;
                }
            },

            _ => {
                // ignore commands not handled by wifi manager
            }
        }
    }
}
