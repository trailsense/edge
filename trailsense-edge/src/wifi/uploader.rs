use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer};

use crate::wifi::{self, WifiCtx, manager::WifiCmd};

#[embassy_executor::task]

pub async fn uploader_task(
    context: WifiCtx,
    wifi_command_sender: Sender<'static, CriticalSectionRawMutex, WifiCmd, 4>,
) {
    loop {
        wifi_command_sender.send(WifiCmd::StopSniffing).await;
        wifi::wait_for_connection(context.stack).await;
        wifi::http::send_data(context.stack, context.tls_seed).await;
        Timer::after(Duration::from_secs(10)).await;
        wifi_command_sender.send(WifiCmd::StartSniffing).await;
        Timer::after(Duration::from_secs(10)).await;
    }
}
