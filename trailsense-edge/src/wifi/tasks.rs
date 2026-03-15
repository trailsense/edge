use embassy_net::Runner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver, pubsub::Publisher,
};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{ClientConfig, ModeConfig, WifiController, WifiDevice, WifiStaState};
use log::{error, info};

use crate::orchestration::types::{CorrelationId, SystemEvents, TransportEvents};

const SSID: Option<&'static str> = option_env!("WIFI_SSID");
const PASSWORD: Option<&'static str> = option_env!("WIFI_PASSWORD");
const WIFI_RETRY_DELAY: Duration = Duration::from_secs(5);
const WIFI_POLL_INTERVAL: Duration = Duration::from_millis(500);
const RECONNECT_SETTLE_DELAY: Duration = Duration::from_secs(2);
const RESTART_SETTLE_DELAY: Duration = Duration::from_secs(2);
const CONNECT_FAILURE_RESTART_THRESHOLD: u8 = 6;

#[derive(Clone, Copy, PartialEq)]
pub enum WifiControlCmd {
    Reconnect,
    RestartController,
    SetAutoConnect { enabled: bool, id: CorrelationId },
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
pub async fn connect(
    mut controller: WifiController<'static>,
    control_receiver: Receiver<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
    orchestrator_event_publisher: Publisher<
        'static,
        CriticalSectionRawMutex,
        SystemEvents,
        4,
        1,
        3,
    >,
) {
    let mut auto_connect_enabled = true;
    let mut last_auto_connect_state = true;
    let mut consecutive_connect_failures: u8 = 0;
    let mut pending_transport_ack: Option<(CorrelationId, bool)> = None;

    let ssid = match SSID {
        Some(v) => v,
        None => {
            error!("WIFI_SSID not set");
            return;
        }
    };

    let password = match PASSWORD {
        Some(v) => v,
        None => {
            error!("WIFI_PASSWORD not set");
            return;
        }
    };

    info!("Connecting to wifi");

    loop {
        let mut requested_reconnect = false;
        let mut requested_restart = false;

        // Drain queued control commands first so state changes are applied promptly.
        while let Ok(cmd) = control_receiver.try_receive() {
            match cmd {
                WifiControlCmd::SetAutoConnect { enabled, id } => {
                    auto_connect_enabled = enabled;
                    info!("WIFI: auto-connect set to {}", enabled);
                    consecutive_connect_failures = 0;
                    pending_transport_ack = Some((id, enabled));
                }
                WifiControlCmd::Reconnect => {
                    info!("Wi-Fi reconnect requested");
                    consecutive_connect_failures = 0;
                    requested_reconnect = true;
                }
                WifiControlCmd::RestartController => {
                    info!("Wi-Fi controller restart requested");
                    consecutive_connect_failures = 0;
                    requested_restart = true;
                }
            }
        }

        if last_auto_connect_state && !auto_connect_enabled {
            let disconnect_ok = match controller.disconnect_async().await {
                Ok(()) => true,
                Err(e) => {
                    error!("WIFI: disconnect on pause failed: {:?}", e);
                    false
                }
            };

            if disconnect_ok {
                if let Some((id, enabled)) = pending_transport_ack {
                    if !enabled {
                        orchestrator_event_publisher
                            .publish(SystemEvents::Transport {
                                id,
                                event: TransportEvents::TransportDisabled,
                            })
                            .await;

                        pending_transport_ack = None;
                    }
                }
            }
        }

        last_auto_connect_state = auto_connect_enabled;

        if !auto_connect_enabled {
            Timer::after(WIFI_POLL_INTERVAL).await;
            continue;
        }

        if auto_connect_enabled && let Some((id, enabled)) = pending_transport_ack {
            if enabled {
                orchestrator_event_publisher
                    .publish(SystemEvents::Transport {
                        id,
                        event: TransportEvents::TransportEnabled,
                    })
                    .await;

                pending_transport_ack = None;
            }
        }

        // This is on top to give it priority over reconnect.
        if requested_restart {
            if let Err(e) = controller.disconnect_async().await {
                error!("Failed to disconnect Wi-Fi before restart: {:?}", e);
            }
            if let Err(e) = controller.stop_async().await {
                error!("Failed to stop Wi-Fi controller: {:?}", e);
            }
            Timer::after(RESTART_SETTLE_DELAY).await;
            continue;
        }

        if requested_reconnect {
            if let Err(e) = controller.disconnect_async().await {
                error!("Failed to disconnect Wi-Fi during reconnect: {:?}", e);
            }
            Timer::after(RECONNECT_SETTLE_DELAY).await;
            continue;
        }

        if matches!(esp_radio::wifi::sta_state(), WifiStaState::Connected) {
            Timer::after(WIFI_POLL_INTERVAL).await;
            continue;
        }

        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(ssid.into())
                    .with_password(password.into()),
            );

            if let Err(e) = controller.set_config(&client_config) {
                error!("Failed to configure wifi client: {:?}", e);
                Timer::after(WIFI_RETRY_DELAY).await;
                continue;
            }

            if let Err(e) = controller.start_async().await {
                error!("Failed to start wifi controller: {:?}", e);
                Timer::after(WIFI_RETRY_DELAY).await;
                continue;
            }
        }

        match controller.connect_async().await {
            Ok(_) => {
                consecutive_connect_failures = 0;
                info!("Wifi connected!");
                Timer::after(WIFI_POLL_INTERVAL).await;
            }
            Err(e) => {
                consecutive_connect_failures = consecutive_connect_failures.saturating_add(1);
                error!(
                    "WIFI: connect_async failed: {:?}, sta_state={:?}, started={:?}, failures={}",
                    e,
                    esp_radio::wifi::sta_state(),
                    controller.is_started(),
                    consecutive_connect_failures
                );

                if consecutive_connect_failures >= CONNECT_FAILURE_RESTART_THRESHOLD {
                    error!(
                        "WIFI: too many connect failures ({}), restarting controller",
                        consecutive_connect_failures
                    );

                    if let Err(err) = controller.disconnect_async().await {
                        error!("WIFI: disconnect before auto-restart failed: {:?}", err);
                    }
                    if let Err(err) = controller.stop_async().await {
                        error!("WIFI: stop during auto-restart failed: {:?}", err);
                    }

                    consecutive_connect_failures = 0;
                    Timer::after(RESTART_SETTLE_DELAY).await;
                    continue;
                }

                Timer::after(WIFI_RETRY_DELAY).await;
            }
        }
    }
}
