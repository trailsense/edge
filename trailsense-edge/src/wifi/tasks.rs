use embassy_net::Runner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{ClientConfig, ModeConfig, WifiController, WifiDevice, WifiStaState};
use log::{error, info};

const SSID: Option<&'static str> = option_env!("WIFI_SSID");
const PASSWORD: Option<&'static str> = option_env!("WIFI_PASSWORD");
const WIFI_RETRY_DELAY: Duration = Duration::from_secs(5);
const WIFI_POLL_INTERVAL: Duration = Duration::from_millis(500);
const RECONNECT_SETTLE_DELAY: Duration = Duration::from_secs(2);
const RESTART_SETTLE_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq)]
pub enum WifiControlCmd {
    Reconnect,
    RestartController,
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
pub async fn connect(
    mut controller: WifiController<'static>,
    control_receiver: Receiver<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
) {
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
        if let Ok(cmd) = control_receiver.try_receive() {
            if cmd == WifiControlCmd::Reconnect {
                info!("Wi-Fi reconnect requested");
                if let Err(e) = controller.disconnect_async().await {
                    error!("Failed to disconnect Wi-Fi during reconnect: {:?}", e);
                }
                Timer::after(RECONNECT_SETTLE_DELAY).await;
            } else if cmd == WifiControlCmd::RestartController {
                info!("Wi-Fi controller restart requested");
                if let Err(e) = controller.disconnect_async().await {
                    error!("Failed to disconnect Wi-Fi before restart: {:?}", e);
                }
                if let Err(e) = controller.stop_async().await {
                    error!("Failed to stop Wi-Fi controller: {:?}", e);
                }
                Timer::after(RESTART_SETTLE_DELAY).await;
            }
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
            Ok(_) => info!("Wifi connected!"),
            Err(e) => {
                error!("Failed to connect to wifi: {:?}", e);
                Timer::after(WIFI_RETRY_DELAY).await;
            }
        }
    }
}
