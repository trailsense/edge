extern crate alloc;

#[cfg(feature = "uplink-gsm")]
use alloc::boxed::Box;
#[cfg(feature = "uplink-gsm")]
use core::ptr::NonNull;
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
struct OwnedRadioController {
    ptr: NonNull<Controller<'static>>,
}

#[cfg(feature = "uplink-gsm")]
impl OwnedRadioController {
    fn new(controller: Controller<'static>) -> (Self, &'static mut Controller<'static>) {
        let radio_ref: &'static mut Controller<'static> = Box::leak(Box::new(controller));
        let ptr = NonNull::from(&mut *radio_ref);
        (Self { ptr }, radio_ref)
    }
}

#[cfg(feature = "uplink-gsm")]
impl Drop for OwnedRadioController {
    fn drop(&mut self) {
        // SAFETY: `ptr` comes from `Box::leak` in `OwnedRadioController::new`
        // and this owner guarantees the allocation is reclaimed exactly once.
        unsafe { drop(Box::from_raw(self.ptr.as_ptr())) };
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
    orchestrator_event_publisher: Publisher<
        'static,
        CriticalSectionRawMutex,
        SystemEvents,
        4,
        1,
        3,
    >,
) {
    let mut radio: Option<OwnedRadioController> = None;
    let mut controller: Option<WifiController<'static>> = None;
    let mut sniffer: Option<Sniffer<'static>> = None;

    loop {
        let command = sniffer_command_receiver.receive().await;

        match command {
            SystemCmd::StartSniffing { id } => {
                if radio.is_none() || controller.is_none() || sniffer.is_none() {
                    match create_gsm_sniffer_runtime().await {
                        Ok((new_radio, new_controller, new_sniffer)) => {
                            radio = Some(new_radio);
                            controller = Some(new_controller);
                            sniffer = Some(new_sniffer);
                        }
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

                let Some(active_sniffer) = sniffer.as_mut() else {
                    orchestrator_event_publisher
                        .publish(SystemEvents::Sniffer {
                            id,
                            event: SnifferEvents::SniffingError,
                        })
                        .await;
                    continue;
                };

                match active_sniffer.set_promiscuous_mode(true) {
                    Ok(()) => {
                        info!("Enabled Promiscuous Mode");
                        active_sniffer.set_receive_cb(callback);
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
                if let Some(active_sniffer) = sniffer.as_mut() {
                    match active_sniffer.set_promiscuous_mode(false) {
                        Ok(()) => info!("Disabled Promiscuous mode"),
                        Err(e) => error!("Failed to disable promiscuous mode: {:?}", e),
                    }
                } else {
                    info!("GSM radio control: sniffer already inactive before upload");
                }

                Timer::after(GSM_PROMISCUOUS_DISABLE_SETTLE_DELAY).await;

                let active_sniffer = sniffer.take();
                let mut active_controller = controller.take();
                let active_radio = radio.take();

                if let Some(controller_ref) = active_controller.as_mut() {
                    info!("GSM radio control: fully deinitializing Wi-Fi for GSM upload");
                    if let Err(e) = controller_ref.stop_async().await {
                        info!(
                            "GSM radio control: stop_async before deinit returned: {:?}",
                            e
                        );
                    }
                }

                drop(active_sniffer);
                drop(active_controller);
                drop(active_radio);

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
async fn create_gsm_sniffer_runtime() -> Result<
    (
        OwnedRadioController,
        WifiController<'static>,
        Sniffer<'static>,
    ),
    WifiError,
> {
    let radio = esp_radio::init()
        .map_err(|_| WifiError::InternalError(esp_radio::wifi::InternalWifiError::State))?;
    let (radio, radio_ref) = OwnedRadioController::new(radio);
    let (mut controller, interfaces) = match esp_radio::wifi::new(
        radio_ref,
        // TODO(hw): avoid `WIFI::steal()` by wiring owned WIFI peripheral from `main`.
        unsafe { WIFI::steal() },
        Default::default(),
    ) {
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

    Ok((radio, controller, interfaces.sniffer))
}

#[cfg(feature = "uplink-gsm")]
fn gsm_sniffer_mode_config() -> ModeConfig {
    ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(GSM_SNIFFER_SSID.into())
            .with_auth_method(AuthMethod::None),
    )
}
