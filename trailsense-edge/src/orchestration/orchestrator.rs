extern crate alloc;

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender, pubsub::Subscriber,
};
use embassy_time::{Duration, Timer};
use log::{debug, error, info};

use crate::orchestration::types::{
    CorrelationId, DataEvents, SnifferEvents, SystemCmd, SystemEvents, SystemState,
    TransportEvents, UploadEvents,
};

const PERIOD: Duration = Duration::from_secs(20); // Change for testing reasons.
const NETWORK_LIMIT: u8 = 5;
const MAX_LOCAL_SAVES: u8 = 10;
const MAX_SAVE_FAILURES: u8 = 5;
const GENERAL_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_SNIFF_TIMEOUT: Duration = Duration::from_secs(8);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(35);

enum WaitResult<T> {
    Matched(T),
    Timeout,
}

// Waits until a matching event arrives; ignores unrelated events.
async fn wait_for<T, F>(
    sub: &mut Subscriber<'static, CriticalSectionRawMutex, SystemEvents, 4, 1, 2>,
    timeout: Duration,
    mut map: F,
) -> WaitResult<T>
where
    F: FnMut(SystemEvents) -> Option<T>,
{
    loop {
        match select(Timer::after(timeout), sub.next_message_pure()).await {
            Either::First(_) => return WaitResult::Timeout,
            Either::Second(ev) => {
                if let Some(v) = map(ev) {
                    return WaitResult::Matched(v);
                } else {
                    // unrelated event from mixed stream; ignore
                    debug!("FSM: ignored unrelated event while waiting");
                }
            }
        }
    }
}

/// # Node Orchestrator Task
///
/// This task handles state changes and instructs different parts of the system to do their part of the work
///
/// **Little Side Note:** I think the main function should do the hardware initialization, and crash early if necessary etc.
/// As soon as all the hardware is initialized, the orchestrator can take over and handle the state. Technically the state coming in should always be idle.
#[embassy_executor::task]
pub async fn orchestrate_node(
    mut event_subscriber: Subscriber<'static, CriticalSectionRawMutex, SystemEvents, 4, 1, 2>,
    network_sender: Sender<'static, CriticalSectionRawMutex, SystemCmd, 4>,
    sniffer_sender: Sender<'static, CriticalSectionRawMutex, SystemCmd, 4>,
    state: SystemState,
) {
    let mut current_state = state;
    let mut network_error_count = 0;
    let mut save_failure_count = 0;
    let mut network_fallback_count = 0;
    let mut uplink_transport_enabled = false;

    let mut next_id: u32 = 1;
    let mut new_id = || {
        let id = CorrelationId(next_id);
        next_id = next_id.wrapping_add(1);
        id
    };

    loop {
        match current_state {
            SystemState::Idle => {
                // TODO: add any further init or whatever else is needed to be here.
                current_state = SystemState::Sniffing;
            }
            SystemState::Sniffing => {
                let transport_id: CorrelationId = new_id();
                info!(
                    "FSM: state=Sniffing send StartSniffing id={}",
                    transport_id.0
                );

                network_sender
                    .send(SystemCmd::SetTransportEnabled {
                        id: transport_id,
                        enabled: false,
                    })
                    .await;

                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Transport {
                        id: ev_id,
                        event: TransportEvents::TransportDisabled,
                    } if ev_id == transport_id => Some(()),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(()) => {}
                    WaitResult::Timeout => {
                        error!("FSM: timeout waiting for transport disable ack");
                        current_state = SystemState::Sniffing;
                        continue;
                    }
                }

                uplink_transport_enabled = false;

                let sniffing_id = new_id();
                sniffer_sender
                    .send(SystemCmd::StartSniffing { id: sniffing_id })
                    .await;
                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Sniffer {
                        id: ev_id,
                        event: SnifferEvents::StartedSniffing,
                    } if ev_id == sniffing_id => Some(true),
                    SystemEvents::Sniffer {
                        id: ev_id,
                        event: SnifferEvents::SniffingError,
                    } if ev_id == sniffing_id => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => {
                        match wait_for(&mut event_subscriber, PERIOD, |e| match e {
                            SystemEvents::Sniffer {
                                id: ev_id,
                                event: SnifferEvents::SniffingError,
                            } if ev_id == sniffing_id => Some(()),
                            _ => None,
                        })
                        .await
                        {
                            WaitResult::Timeout => current_state = SystemState::PreparingUpload, // period done
                            WaitResult::Matched(()) => current_state = SystemState::Sniffing, // recover/retry
                        }
                    }
                    WaitResult::Matched(false) | WaitResult::Timeout => {
                        current_state = SystemState::Sniffing; // retry policy
                    }
                }
            }
            SystemState::PreparingUpload => {
                let sniffing_id = new_id();
                info!(
                    "FSM: state=PreparingUpload send StopSniffing id={}",
                    sniffing_id.0
                );

                sniffer_sender
                    .send(SystemCmd::StopSniffing { id: sniffing_id })
                    .await;

                match wait_for(&mut event_subscriber, STOP_SNIFF_TIMEOUT, |e| match e {
                    SystemEvents::Sniffer {
                        id: ev_id,
                        event: SnifferEvents::StoppedSniffing,
                    } if ev_id == sniffing_id => Some(true),
                    SystemEvents::Sniffer {
                        id: ev_id,
                        event: SnifferEvents::SniffingError,
                    } if ev_id == sniffing_id => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => current_state = SystemState::Connecting,
                    WaitResult::Matched(false) => current_state = SystemState::Sniffing,
                    WaitResult::Timeout => current_state = SystemState::Sniffing,
                }

                let transport_id = new_id();

                network_sender
                    .send(SystemCmd::SetTransportEnabled {
                        id: transport_id,
                        enabled: true,
                    })
                    .await;

                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Transport {
                        id: ev_id,
                        event: TransportEvents::TransportEnabled,
                    } if ev_id == transport_id => Some(()),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(()) => {
                        uplink_transport_enabled = true;
                    }
                    WaitResult::Timeout => {
                        error!("FSM: timeout waiting for transport enable ack");
                        current_state = SystemState::Sniffing;
                        continue;
                    }
                }
            }
            SystemState::Connecting => {
                let id = new_id();
                info!(
                    "FSM: state=Connecting send Connect id={} retry={}",
                    id.0, network_error_count
                );
                network_sender.send(SystemCmd::Connect { id }).await;

                match wait_for(&mut event_subscriber, CONNECT_TIMEOUT, |e| match e {
                    SystemEvents::Upload {
                        id: ev_id,
                        event: UploadEvents::NetworkConnected,
                    } if ev_id == id => Some(true),
                    SystemEvents::Upload {
                        id: ev_id,
                        event: UploadEvents::NetworkError,
                    } if ev_id == id => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => current_state = SystemState::Uploading,
                    WaitResult::Matched(false) | WaitResult::Timeout => {
                        network_error_count += 1;
                        current_state = if network_error_count >= NETWORK_LIMIT {
                            SystemState::SavingData
                        } else {
                            Timer::after(Duration::from_secs(2)).await;
                            SystemState::Connecting
                        };
                    }
                }
            }
            SystemState::Uploading => {
                let id = new_id();
                info!(
                    "FSM: state=Uploading send UploadData id={} retry={}",
                    id.0, network_error_count
                );
                network_sender.send(SystemCmd::UploadData { id }).await;
                match wait_for(&mut event_subscriber, UPLOAD_TIMEOUT, |e| match e {
                    SystemEvents::Upload {
                        id: ev_id,
                        event: UploadEvents::UploadSuccessful,
                    } if ev_id == id => Some(Ok(())),
                    SystemEvents::Upload {
                        id: ev_id,
                        event: UploadEvents::UploadError,
                    }
                    | SystemEvents::Upload {
                        id: ev_id,
                        event: UploadEvents::NetworkError,
                    } if ev_id == id => Some(Err(())),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(Ok(())) => {
                        network_error_count = 0;
                        save_failure_count = 0;
                        network_fallback_count = 0;
                        current_state = SystemState::Idle;
                    }
                    WaitResult::Matched(Err(())) | WaitResult::Timeout => {
                        network_error_count += 1;
                        current_state = if network_error_count >= NETWORK_LIMIT {
                            SystemState::SavingData
                        } else {
                            SystemState::Connecting
                        };
                    }
                }
            }
            SystemState::SavingData => {
                if uplink_transport_enabled {
                    let transport_id = new_id();

                    network_sender
                        .send(SystemCmd::SetTransportEnabled {
                            id: transport_id,
                            enabled: false,
                        })
                        .await;

                    match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                        SystemEvents::Transport {
                            id: ev_id,
                            event: TransportEvents::TransportDisabled,
                        } if ev_id == transport_id => Some(()),
                        _ => None,
                    })
                    .await
                    {
                        WaitResult::Matched(()) => {
                            uplink_transport_enabled = false;
                        }
                        WaitResult::Timeout => {
                            error!("FSM: timeout waiting for transport disable in SavingData");
                        }
                    }
                }

                let id = new_id();
                info!(
                    "FSM: state=SavingData send SaveLocally id={} save_failures={}",
                    id.0, save_failure_count
                );
                network_sender.send(SystemCmd::SaveLocally { id }).await;

                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Data {
                        id: ev_id,
                        event: DataEvents::DataSaved,
                    } if ev_id == id => Some(true),
                    SystemEvents::Data {
                        id: ev_id,
                        event: DataEvents::DataError,
                    } if ev_id == id => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => {
                        current_state = SystemState::Idle;
                        network_fallback_count += 1;
                        save_failure_count = 0;
                    }
                    WaitResult::Matched(false) | WaitResult::Timeout => {
                        error!("Issue saving data locally");
                        save_failure_count += 1;
                        current_state = SystemState::SavingData;
                    }
                }

                if network_fallback_count >= MAX_LOCAL_SAVES
                    || save_failure_count >= MAX_SAVE_FAILURES
                {
                    network_fallback_count = 0;
                    save_failure_count = 0;
                    current_state = SystemState::Sleep;
                }
            }
            SystemState::Sleep => {
                // TODO: Implement real deep sleep. For now just take a X min break.
                Timer::after(PERIOD).await;
                current_state = SystemState::Idle;
            }
        }
    }
}
