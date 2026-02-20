use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};

use crate::{
    packages::package_store,
    probes::{counter, fingerprint_store},
    wifi::{self, WifiCtx, manager::WifiCmd},
};

#[embassy_executor::task]
pub async fn uploader_task(
    context: WifiCtx,
    wifi_command_sender: Sender<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    wifi_command_sender.send(WifiCmd::StartSniffing).await;

    const PERIOD: Duration = Duration::from_secs(300);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    const SEND_TIMEOUT: Duration = Duration::from_secs(30);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    const RADIO_SETTLE_DELAY: Duration = Duration::from_secs(1);
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

        let fingerprint_snapshot = fingerprint_store::snapshot();
        let curr_count = counter::deduplicate_probes(&fingerprint_snapshot);
        package_store::push(curr_count).await; // TODO: implement limit to avoid buffer overflow of http request. Basically use chunking.
        fingerprint_store::drain();

        wifi_command_sender.send(WifiCmd::StopSniffing).await;
        Timer::after(RADIO_SETTLE_DELAY).await;

        let mut ok = false;
        for attempt in 0..SEND_ATTEMPTS {
            let packages = package_store::snapshot_with_age().await;

            match wifi::http::send_data(context.stack, context.tls_seed, packages)
                .with_timeout(SEND_TIMEOUT)
                .await
            {
                Ok(true) => {
                    package_store::drain().await;
                    ok = true;
                    break;
                }
                Ok(false) => error!("HTTP send failed"),
                Err(_) => error!("Package sending timed out"),
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
