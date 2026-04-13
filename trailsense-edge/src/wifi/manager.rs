extern crate alloc;

use embassy_sync::channel::Receiver;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, pubsub::Publisher};

use embassy_time::{Duration, Timer};
#[cfg(feature = "uplink-gsm")]
use esp_hal::peripherals::WIFI;
#[cfg(feature = "uplink-gsm")]
use esp_radio::Controller;
#[cfg(feature = "uplink-gsm")]
use esp_radio::wifi::{AuthMethod, ClientConfig, ModeConfig, WifiController, WifiError};
use esp_radio::wifi::{PromiscuousPkt, Sniffer};
use log::{error, info};

use crate::orchestration::types::{SnifferEvents, SystemCmd, SystemEvents};

const RADIO_SETTLE_DELAY: Duration = Duration::from_secs(5);
#[cfg(feature = "uplink-gsm")]
const GSM_PROMISCUOUS_DISABLE_SETTLE_DELAY: Duration = Duration::from_millis(500);
#[cfg(feature = "uplink-gsm")]
const GSM_WIFI_STOP_SETTLE_DELAY: Duration = Duration::from_secs(5);
#[cfg(feature = "uplink-gsm")]
const GSM_WIFI_START_SETTLE_DELAY: Duration = Duration::from_secs(2);
#[cfg(feature = "uplink-gsm")]
const GSM_SNIFFER_SSID: &str = "trailsense-sniffer";

#[cfg(feature = "uplink-gsm")]
static GSM_RADIO_CELL: static_cell::StaticCell<Controller<'static>> =
    static_cell::StaticCell::new();

#[cfg(feature = "uplink-gsm")]
struct GsmSnifferRuntime {
    controller: WifiController<'static>,
    sniffer: Sniffer<'static>,
}

#[cfg(feature = "uplink-gsm")]
impl GsmSnifferRuntime {
    fn new(controller: WifiController<'static>, sniffer: Sniffer<'static>) -> Self {
        GsmSnifferRuntime {
            controller,
            sniffer,
        }
    }
}

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

#[cfg(feature = "uplink-gsm")]
#[embassy_executor::task]
pub async fn gsm_wifi_manager_task(
    callback: fn(PromiscuousPkt),
    sniffer_command_receiver: Receiver<'static, CriticalSectionRawMutex, SystemCmd, 4>,
    wifi_peripheral: WIFI<'static>,
    orchestrator_event_publisher: Publisher<
        'static,
        CriticalSectionRawMutex,
        SystemEvents,
        4,
        1,
        3,
    >,
) {
    let mut wifi_peripheral = Some(wifi_peripheral);
    let mut gsm_sniffer_runtime: Option<GsmSnifferRuntime> = None;

    loop {
        let command = sniffer_command_receiver.receive().await;

        match command {
            SystemCmd::StartSniffing { id } => {
                if gsm_sniffer_runtime.is_none() {
                    let Some(wifi) = wifi_peripheral.take() else {
                        error!(
                            "GSM radio control: WIFI peripheral already consumed; cannot create runtime"
                        );
                        orchestrator_event_publisher
                            .publish(SystemEvents::Sniffer {
                                id,
                                event: SnifferEvents::SniffingError,
                            })
                            .await;
                        continue;
                    };

                    match create_gsm_sniffer_runtime(wifi).await {
                        Ok(gsr) => gsm_sniffer_runtime = Some(gsr),
                        Err(e) => {
                            error!(
                                "GSM radio control: failed to create Wi-Fi/sniffer runtime: {:?}",
                                e
                            );
                            orchestrator_event_publisher
                                .publish(SystemEvents::Sniffer {
                                    id,
                                    event: SnifferEvents::SniffingError,
                                })
                                .await;
                            continue;
                        }
                    }
                }

                let active_runtime = match gsm_sniffer_runtime.as_mut() {
                    Some(rt) => rt,
                    None => {
                        orchestrator_event_publisher
                            .publish(SystemEvents::Sniffer {
                                id,
                                event: SnifferEvents::SniffingError,
                            })
                            .await;
                        continue;
                    }
                };

                match active_runtime.controller.is_started() {
                    Ok(true) => {}
                    Ok(false) => {
                        if let Err(e) = active_runtime
                            .controller
                            .set_config(&gsm_sniffer_mode_config())
                        {
                            error!(
                                "GSM radio control: failed to configure Wi-Fi for sniffing: {:?}",
                                e
                            );
                            orchestrator_event_publisher
                                .publish(SystemEvents::Sniffer {
                                    id,
                                    event: SnifferEvents::SniffingError,
                                })
                                .await;
                            continue;
                        }

                        if let Err(e) = active_runtime.controller.start_async().await {
                            error!(
                                "GSM radio control: failed to start Wi-Fi for sniffing: {:?}",
                                e
                            );
                            orchestrator_event_publisher
                                .publish(SystemEvents::Sniffer {
                                    id,
                                    event: SnifferEvents::SniffingError,
                                })
                                .await;
                            continue;
                        }

                        Timer::after(GSM_WIFI_START_SETTLE_DELAY).await;
                    }
                    Err(e) => {
                        error!("GSM radio control: is_started failed: {:?}", e);
                        orchestrator_event_publisher
                            .publish(SystemEvents::Sniffer {
                                id,
                                event: SnifferEvents::SniffingError,
                            })
                            .await;
                        continue;
                    }
                }

                match active_runtime.sniffer.set_promiscuous_mode(true) {
                    Ok(()) => {
                        info!("Enabled Promiscuous Mode");
                        active_runtime.sniffer.set_receive_cb(callback);
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
                }
            }

            SystemCmd::StopSniffing { id } => {
                if let Some(rt) = gsm_sniffer_runtime.as_mut() {
                    match rt.sniffer.set_promiscuous_mode(false) {
                        Ok(()) => info!("Disabled Promiscuous mode"),
                        Err(e) => error!("Failed to disable promiscuous mode: {:?}", e),
                    }
                } else {
                    info!("GSM radio control: sniffer already inactive before upload");
                }

                Timer::after(GSM_PROMISCUOUS_DISABLE_SETTLE_DELAY).await;

                if let Some(rt) = gsm_sniffer_runtime.as_mut() {
                    info!("GSM radio control: fully deinitializing Wi-Fi for GSM upload");
                    if let Err(e) = rt.controller.stop_async().await {
                        info!(
                            "GSM radio control: stop_async before deinit returned: {:?}",
                            e
                        );
                    }
                }

                Timer::after(GSM_WIFI_STOP_SETTLE_DELAY).await;

                orchestrator_event_publisher
                    .publish(SystemEvents::Sniffer {
                        id,
                        event: SnifferEvents::StoppedSniffing,
                    })
                    .await;
            }

            _ => {
                // ignore commands not handled by wifi manager
            }
        }
    }
}

#[cfg(feature = "uplink-gsm")]
async fn create_gsm_sniffer_runtime(wifi: WIFI<'static>) -> Result<GsmSnifferRuntime, WifiError> {
    let radio = esp_radio::init()
        .map_err(|_| WifiError::InternalError(esp_radio::wifi::InternalWifiError::State))?;

    let radio_ref = GSM_RADIO_CELL
        .try_init(radio)
        .ok_or(WifiError::InternalError(
            esp_radio::wifi::InternalWifiError::State,
        ))?;
    let (mut controller, interfaces) =
        match esp_radio::wifi::new(radio_ref, wifi, Default::default()) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

    if let Err(e) = controller.set_config(&gsm_sniffer_mode_config()) {
        return Err(e);
    }
    info!("GSM radio control: starting Wi-Fi controller for sniffing");
    if let Err(e) = controller.start_async().await {
        return Err(e);
    }
    Timer::after(GSM_WIFI_START_SETTLE_DELAY).await;

    Ok(GsmSnifferRuntime::new(controller, interfaces.sniffer))
}

#[cfg(feature = "uplink-gsm")]
fn gsm_sniffer_mode_config() -> ModeConfig {
    ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(GSM_SNIFFER_SSID.into())
            .with_auth_method(AuthMethod::None),
    )
}
