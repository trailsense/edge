use embassy_net::Runner;
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    ClientConfig, ModeConfig, WifiController, WifiDevice, WifiEvent, WifiStaState,
};
use log::{error, info};

const SSID: Option<&'static str> = option_env!("WIFI_SSID");
const PASSWORD: Option<&'static str> = option_env!("WIFI_PASSWORD");

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
pub async fn connect(mut controller: WifiController<'static>) {
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
        if matches!(esp_radio::wifi::sta_state(), WifiStaState::Connected) {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_millis(5000)).await;
        }

        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(ssid.into())
                    .with_password(password.into()),
            );

            if let Err(e) = controller.set_config(&client_config) {
                error!("Failed to configure wifi client: {:?}", e);
                Timer::after(Duration::from_millis(5000)).await;
                continue;
            }

            if let Err(e) = controller.start_async().await {
                error!("Failed to start wifi controller: {:?}", e);
                Timer::after(Duration::from_millis(5000)).await;
                continue;
            }
        }

        match controller.connect_async().await {
            Ok(_) => info!("Wifi connected!"),
            Err(e) => {
                error!("Failed to connect to wifi: {:?}", e);
                Timer::after(Duration::from_millis(5000)).await;
            }
        }
    }
}
