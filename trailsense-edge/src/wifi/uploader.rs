use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};

use crate::wifi::{self, WifiCtx, manager::WifiCmd};

#[embassy_executor::task]
pub async fn uploader_task(
    context: WifiCtx,
    wifi_command_sender: Sender<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    wifi_command_sender.send(WifiCmd::StartSniffing).await;

    const PERIOD: Duration = Duration::from_secs(10);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    const SEND_TIMEOUT: Duration = Duration::from_secs(5);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    const SEND_ATTEMPTS: u8 = 2;

    loop {
        Timer::after(PERIOD).await;

        if wifi::wait_for_connection(context.stack)
            .with_timeout(CONNECT_TIMEOUT)
            .await
            .is_err()
        {
            error!("WiFi connection timeout");
            continue;
        }

        wifi_command_sender.send(WifiCmd::StopSniffing).await;

        let mut ok = false;
        for attempt in 0..SEND_ATTEMPTS {
            if wifi::http::send_data(context.stack, context.tls_seed)
                .with_timeout(SEND_TIMEOUT)
                .await
                .is_ok()
            {
                ok = true;
                break;
            }

            if attempt + 1 < SEND_ATTEMPTS {
                Timer::after(RETRY_DELAY).await;
            }
        }

        wifi_command_sender.send(WifiCmd::StartSniffing).await;

        if ok {
            info!("Package sent successfully");
        } else {
            error!("Package sending failed");
        }
    }
}
