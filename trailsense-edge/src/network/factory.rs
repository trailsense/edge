use crate::network::active_transport::ActiveTransport;
#[cfg(feature = "uplink-gsm")]
use crate::network::gsm::transport::GsmTransport;
#[cfg(feature = "uplink-wifi")]
use crate::wifi::WifiCtx;
#[cfg(feature = "uplink-wifi")]
use crate::wifi::tasks::WifiControlCmd;
#[cfg(feature = "uplink-gsm")]
use embassy_executor::Spawner;
#[cfg(feature = "uplink-wifi")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
#[cfg(feature = "uplink-gsm")]
use esp_hal::{Async, uart::Uart};

#[cfg(feature = "uplink-wifi")]
pub fn build_active_transport(
    ctx: WifiCtx,
    wifi_control_sender: Sender<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
) -> ActiveTransport {
    use crate::network::wifi::transport::{WifiTransport, WifiTransportConfig};

    let config = WifiTransportConfig::default();
    return ActiveTransport::Wifi(WifiTransport::new(ctx, config, wifi_control_sender));
}

#[cfg(feature = "uplink-gsm")]
pub fn build_active_transport(uart: Uart<'static, Async>, spawner: Spawner) -> ActiveTransport {
    return ActiveTransport::Gsm(GsmTransport::new(uart, spawner));
}
