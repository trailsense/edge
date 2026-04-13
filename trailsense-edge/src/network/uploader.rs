extern crate alloc;

use crate::{
    network::{
        TransportControl, UplinkTransport,
        types::{ConnectionOutcome, ControlOutcome},
    },
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
        3,
    >,
) {
    #[cfg(feature = "uplink-gsm")]
    const SEND_TIMEOUT: Duration = Duration::from_secs(90);
    #[cfg(not(feature = "uplink-gsm"))]
    const SEND_TIMEOUT: Duration = Duration::from_secs(30);
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    // TODO(recovery): track consecutive GSM failures and trigger modem reset/rebootstrap after N failures instead of immediate terminal behavior (hardware reset).
    #[cfg(feature = "uplink-gsm")]
    const SEND_ATTEMPTS: u8 = 1;
    #[cfg(not(feature = "uplink-gsm"))]
    const SEND_ATTEMPTS: u8 = 5;
    #[cfg(feature = "uplink-gsm")]
    const GSM_RECOVERY_DELAY: Duration = Duration::from_secs(15);

    loop {
        let command = network_command_receiver.receive().await;
        let outcome;
        match command {
            SystemCmd::SetTransportEnabled { id, enabled } => {
                if enabled {
                    outcome = transport.control(TransportControl::Enable, id).await;
                } else {
                    outcome = transport.control(TransportControl::Disable, id).await;
                }

                match outcome {
                    ControlOutcome::Applied => {
                        let event = if enabled {
                            TransportEvents::TransportEnabled
                        } else {
                            TransportEvents::TransportDisabled
                        };

                        orchestrator_event_publisher
                            .publish(SystemEvents::Transport { id, event })
                            .await;
                    }
                    ControlOutcome::PendingExternalAck => {
                        // Wifi control publishes transport acks from its own manager task
                    }
                    ControlOutcome::Failed => {
                        error!(
                            "UPL: transport control failed for id={} enabled={}; publishing failure",
                            id.0, enabled
                        );
                        let event = TransportEvents::TransportControlFailed;

                        orchestrator_event_publisher
                            .publish(SystemEvents::Transport { id, event })
                            .await;
                    }
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
                    #[cfg(feature = "uplink-gsm")]
                    {
                        // Avoid immediately hammering modem HTTP state after failure.
                        Timer::after(GSM_RECOVERY_DELAY).await;
                    }
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
