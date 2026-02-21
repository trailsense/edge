use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};

use crate::{
    packages::package_store,
    probes::{counter, fingerprint_store},
    wifi::{self, WifiCtx, http::SendDataOutcome, manager::WifiCmd, tasks::WifiControlCmd},
};

#[embassy_executor::task]
pub async fn uploader_task(
    context: WifiCtx,
    wifi_command_sender: Sender<'static, CriticalSectionRawMutex, WifiCmd, 4>,
    wifi_control_sender: Sender<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
) {
    wifi_command_sender.send(WifiCmd::StartSniffing).await;

    const PERIOD: Duration = Duration::from_secs(20);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    const SEND_TIMEOUT: Duration = Duration::from_secs(30);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    const RADIO_SETTLE_DELAY: Duration = Duration::from_secs(5);
    const SEND_ATTEMPTS: u8 = 2;
    const DNS_RECONNECT_THRESHOLD: u8 = 2;
    let mut consecutive_dns_failures: u8 = 0;

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
        package_store::push(curr_count); // TODO: implement limit to avoid buffer overflow of http request. Basically use chunking.
        fingerprint_store::drain();

        wifi_command_sender.send(WifiCmd::StopSniffing).await;
        Timer::after(RADIO_SETTLE_DELAY).await;

        let mut ok = false;
        let mut saw_dns_failure = false;
        for attempt in 0..SEND_ATTEMPTS {
            let packages = package_store::snapshot_with_age();

            match wifi::http::send_data(context.stack, context.tls_seed, packages)
                .with_timeout(SEND_TIMEOUT)
                .await
            {
                Ok(SendDataOutcome::Success) => {
                    package_store::drain();
                    ok = true;
                    break;
                }
                Ok(SendDataOutcome::DnsFailure) => {
                    saw_dns_failure = true;
                    error!("HTTP send failed due to DNS resolution");
                }
                Ok(SendDataOutcome::Failure) => error!("HTTP send failed"),
                Err(_) => error!("Package sending timed out"),
            }

            if attempt + 1 < SEND_ATTEMPTS {
                Timer::after(RETRY_DELAY).await;
            }
        }

        if saw_dns_failure {
            consecutive_dns_failures = consecutive_dns_failures.saturating_add(1);
            if consecutive_dns_failures >= DNS_RECONNECT_THRESHOLD {
                error!(
                    "Consecutive DNS failures reached {}; forcing Wi-Fi reconnect",
                    DNS_RECONNECT_THRESHOLD
                );
                wifi_control_sender.send(WifiControlCmd::Reconnect).await;
                consecutive_dns_failures = 0;
            }
        } else if ok {
            consecutive_dns_failures = 0;
        }

        wifi_command_sender.send(WifiCmd::StartSniffing).await;

        if ok {
            info!("Package sent successfully");
        } else {
            error!("Package sending failed");
        }
    }
}
