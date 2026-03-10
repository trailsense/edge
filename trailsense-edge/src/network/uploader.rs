extern crate alloc;

use crate::{
    network::{TransportControl, UplinkTransport, types::ConnectionOutcome},
    orchestration::types::{DataEvents, SystemCmd, SystemEvents, TransportEvents, UploadEvents},
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

        match command {
            SystemCmd::SetTransportEnabled { id, enabled } => {
                if enabled {
                    transport.control(TransportControl::Enable).await;
                    orchestrator_event_publisher
                        .publish(SystemEvents::Transport {
                            id,
                            event: TransportEvents::TransportEnabled,
                        })
                        .await;
                } else {
                    transport.control(TransportControl::Disable).await;
                    orchestrator_event_publisher
                        .publish(SystemEvents::Transport {
                            id,
                            event: TransportEvents::TransportDisabled,
                        })
                        .await;
                }
            }
            SystemCmd::Connect { id } => {
                info!("UPL: recv Connect id={}", id.0);
                match transport.ensure_connected().await {
                    ConnectionOutcome::Connected => {
                        orchestrator_event_publisher
                            .publish(SystemEvents::Upload {
                                id,
                                event: UploadEvents::NetworkConnected,
                            })
                            .await;
                    }
                    ConnectionOutcome::Failure | ConnectionOutcome::Disconnected => {
                        orchestrator_event_publisher
                            .publish(SystemEvents::Upload {
                                id,
                                event: UploadEvents::NetworkError,
                            })
                            .await;
                    }
                }
            }

            SystemCmd::UploadData { id } => {
                info!("UPL: recv UploadData id={}", id.0);
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
                            error!("UPL: id={} data sending had retryable failure", id.0);
                        }
                        Ok(SendDataOutcome::FatalFailure) => {
                            error!("HTTP send failed");
                            ok = false;
                            break;
                        }
                        Ok(SendDataOutcome::BackoffRequired) => {
                            info!("UPL: id={} backoff required; recovery pending", id.0);
                            break;
                        }
                        Err(_) => error!("Package sending timed out"),
                    }

                    if attempt + 1 < SEND_ATTEMPTS {
                        Timer::after(RETRY_DELAY).await;
                    }
                }

                let event = if ok {
                    UploadEvents::UploadSuccessful
                } else {
                    UploadEvents::UploadError
                };

                orchestrator_event_publisher
                    .publish(SystemEvents::Upload { id, event })
                    .await;
            }

            SystemCmd::SaveLocally { id } => {
                info!("UPL: recv SaveLocally id={}", id.0);
                let fingerprint_snapshot = fingerprint_store::snapshot();
                let saved_ok = if fingerprint_snapshot.is_empty() {
                    true
                } else {
                    let curr_count = counter::deduplicate_probes(&fingerprint_snapshot);
                    let ok = package_store::push(curr_count);
                    if ok {
                        fingerprint_store::drain();
                    }
                    ok
                };

                let event = if saved_ok {
                    DataEvents::DataSaved
                } else {
                    DataEvents::DataError
                };

                orchestrator_event_publisher
                    .publish(SystemEvents::Data { id, event })
                    .await;
            }

            _ => {
                // ignore commands not handled by uploader
            }
        }
    }
}
