extern crate alloc;
use crate::network::{UplinkTransport, types::ConnectionOutcome};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};

use crate::{
    network::{active_transport::ActiveTransport, types::SendDataOutcome},
    packages::package_store,
    probes::{counter, fingerprint_store},
    wifi::manager::WifiCmd,
};

#[embassy_executor::task]
pub async fn uploader_task(
    mut transport: ActiveTransport,
    wifi_command_sender: Sender<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    const PERIOD: Duration = Duration::from_secs(20);
    const SEND_TIMEOUT: Duration = Duration::from_secs(30);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    const RADIO_SETTLE_DELAY: Duration = Duration::from_secs(5);
    const SEND_ATTEMPTS: u8 = 5;

    wifi_command_sender.send(WifiCmd::StartSniffing).await;

    loop {
        Timer::after(PERIOD).await;

        match transport.ensure_connected().await {
            ConnectionOutcome::Connected => {
                info!("Connection Established")
            }
            ConnectionOutcome::Failure | ConnectionOutcome::Disconnected => {
                error!("Connection timeout");
                continue;
            }
        }

        let fingerprint_snapshot = fingerprint_store::snapshot();
        let curr_count = counter::deduplicate_probes(&fingerprint_snapshot);
        package_store::push(curr_count); // TODO: implement limit to avoid buffer overflow of http request. Basically use chunking.
        fingerprint_store::drain();

        wifi_command_sender.send(WifiCmd::StopSniffing).await;
        Timer::after(RADIO_SETTLE_DELAY).await;

        let mut ok = false;
        for attempt in 0..SEND_ATTEMPTS {
            let packages = package_store::snapshot_with_age();

            match transport
                .send_data(packages)
                .with_timeout(SEND_TIMEOUT)
                .await
            {
                Ok(SendDataOutcome::Success) => {
                    package_store::drain();
                    ok = true;
                    break;
                }
                Ok(SendDataOutcome::RetryableFailure) => {
                    error!("Data sending had a retriable failure");
                }
                Ok(SendDataOutcome::FatalFailure) => {
                    error!("HTTP send failed");
                    ok = false;
                    break;
                }
                Ok(SendDataOutcome::BackoffRequired) => {
                    info!(
                        "Transport recovery/backoff in progress; skipping remaining attempts this cycle"
                    );
                    break;
                }
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
