extern crate alloc;
use alloc::vec::Vec;

#[cfg(feature = "uplink-wifi")]
use crate::network::wifi::transport::WifiTransport;
use crate::{
    network::{
        UplinkTransport,
        types::{ConnectionOutcome, SendDataOutcome},
    },
    packages::package_store::PackageEntity,
};

#[cfg(feature = "uplink-wifi")]
pub enum ActiveTransport {
    Wifi(WifiTransport),
}

impl UplinkTransport for ActiveTransport {
    async fn ensure_connected(&mut self) -> ConnectionOutcome {
        match self {
            ActiveTransport::Wifi(t) => t.ensure_connected().await,
        }
    }

    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome {
        match self {
            ActiveTransport::Wifi(t) => t.send_data(packages).await,
        }
    }
}
