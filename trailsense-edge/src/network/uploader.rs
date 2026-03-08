extern crate alloc;

use crate::{
    network::{UplinkTransport, types::ConnectionOutcome},
    orchestration::types::{SystemCmd, SystemEvents, UploadEvents},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Receiver, pubsub::Publisher,
};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};

use crate::{
    network::{active_transport::ActiveTransport, types::SendDataOutcome},
    packages::package_store,
    probes::{counter, fingerprint_store},
};

#[embassy_executor::task]
pub async fn uploader_task(
    mut transport: ActiveTransport,
    network_command_receiver: Receiver<'static, CriticalSectionRawMutex, SystemCmd, 4>,
    orchestrator_event_publisher: Publisher<
        'static,
        CriticalSectionRawMutex,
        SystemEvents,
        4,
        1,
        2,
    >,
) {
    const SEND_TIMEOUT: Duration = Duration::from_secs(30);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    const SEND_ATTEMPTS: u8 = 5;

    loop {
        let command = network_command_receiver.receive().await;

        if command == SystemCmd::Connect {
            match transport.ensure_connected().await {
                ConnectionOutcome::Connected => {
                    info!("Connection Established");
                    orchestrator_event_publisher
                        .publish(SystemEvents::Upload(UploadEvents::NetworkConnected))
                        .await;
                    continue;
                }
                ConnectionOutcome::Failure | ConnectionOutcome::Disconnected => {
                    error!("Connection timeout");
                    orchestrator_event_publisher
                        .publish(SystemEvents::Upload(UploadEvents::NetworkError))
                        .await;
                    continue;
                }
            }
        } else if command == SystemCmd::UploadData {
            let fingerprint_snapshot = fingerprint_store::snapshot();
            let curr_count = counter::deduplicate_probes(&fingerprint_snapshot);
            package_store::push(curr_count); // TODO: implement limit to avoid buffer overflow of http request. Basically use chunking.
            fingerprint_store::drain();

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

            if ok {
                orchestrator_event_publisher
                    .publish(SystemEvents::Upload(UploadEvents::UploadSuccessfull))
                    .await;
                info!("Package sent successfully");
            } else {
                orchestrator_event_publisher
                    .publish(SystemEvents::Upload(UploadEvents::UploadError))
                    .await;
                error!("Package sending failed");
            }
        }
    }
}
