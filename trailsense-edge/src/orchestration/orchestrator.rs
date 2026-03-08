extern crate alloc;

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender, pubsub::Subscriber,
};
use embassy_time::{Duration, Timer};
use log::error;

use crate::orchestration::types::{
    DataEvents, SnifferEvents, SystemCmd, SystemEvents, SystemState, UploadEvents,
};

const PERIOD: Duration = Duration::from_secs(180); // Change for testing reasons.
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
                    log::debug!("Ignoring unrelated event while waiting");
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
    loop {
        match current_state {
            SystemState::Idle => {
                // TODO: add any further init or whatever else is needed to be here.
                current_state = SystemState::Sniffing;
            }
            SystemState::Sniffing => {
                sniffer_sender.send(SystemCmd::StartSniffing).await;

                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Sniffer(SnifferEvents::StartedSniffing) => Some(true),
                    SystemEvents::Sniffer(SnifferEvents::SniffingError) => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => {
                        match wait_for(&mut event_subscriber, PERIOD, |e| match e {
                            SystemEvents::Sniffer(SnifferEvents::SniffingError) => Some(()),
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
                sniffer_sender.send(SystemCmd::StopSniffing).await;

                match wait_for(&mut event_subscriber, STOP_SNIFF_TIMEOUT, |e| match e {
                    SystemEvents::Sniffer(SnifferEvents::StoppedSniffing) => Some(true),
                    SystemEvents::Sniffer(SnifferEvents::SniffingError) => Some(false),
                    _ => None,
                })
                .await
                {
                    WaitResult::Matched(true) => current_state = SystemState::Connecting,
                    WaitResult::Matched(false) => current_state = SystemState::Sniffing,
                    WaitResult::Timeout => current_state = SystemState::Sniffing,
                }
            }
            SystemState::Connecting => {
                network_sender.send(SystemCmd::Connect).await;

                match wait_for(&mut event_subscriber, CONNECT_TIMEOUT, |e| match e {
                    SystemEvents::Upload(UploadEvents::NetworkConnected) => Some(true),
                    SystemEvents::Upload(UploadEvents::NetworkError) => Some(false),
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
                            SystemState::Sniffing
                        };
                    }
                }
            }
            SystemState::Uploading => {
                network_sender.send(SystemCmd::UploadData).await;
                match wait_for(&mut event_subscriber, UPLOAD_TIMEOUT, |e| match e {
                    SystemEvents::Upload(UploadEvents::UploadSuccessfull) => Some(Ok(())),
                    SystemEvents::Upload(UploadEvents::UploadError)
                    | SystemEvents::Upload(UploadEvents::NetworkError) => Some(Err(())),
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
                network_sender.send(SystemCmd::SaveLocally).await;

                match wait_for(&mut event_subscriber, GENERAL_TIMEOUT, |e| match e {
                    SystemEvents::Data(DataEvents::DataSaved) => Some(true),
                    SystemEvents::Data(DataEvents::DataError) => Some(false),
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
