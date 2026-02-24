#[cfg(feature = "uplink-wifi")]
use crate::wifi::tasks::WifiControlCmd;
use crate::{network::active_transport::ActiveTransport, wifi::WifiCtx};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};

#[cfg(feature = "uplink-wifi")]
pub fn build_active_transport(
    ctx: WifiCtx,
    wifi_control_sender: Sender<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
) -> ActiveTransport {
    use crate::network::wifi::transport::{WifiTransport, WifiTransportConfig};

    let config = WifiTransportConfig::default();
    return ActiveTransport::Wifi(WifiTransport::new(ctx, config, wifi_control_sender));
}
