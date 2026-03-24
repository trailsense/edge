extern crate alloc;
use alloc::vec::Vec;

#[cfg(feature = "uplink-gsm")]
use crate::network::gsm::transport::GsmTransport;
use crate::network::types::ControlOutcome;
use crate::orchestration::types::CorrelationId;

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

#[cfg(feature = "uplink-wifi")]
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

    async fn control(&mut self, cmd: super::TransportControl, id: CorrelationId) -> ControlOutcome {
        match self {
            ActiveTransport::Wifi(t) => t.control(cmd, id).await,
        }
    }
}

#[cfg(feature = "uplink-gsm")]
pub enum ActiveTransport {
    Gsm(GsmTransport),
}

#[cfg(feature = "uplink-gsm")]
impl UplinkTransport for ActiveTransport {
    async fn ensure_connected(&mut self) -> ConnectionOutcome {
        match self {
            ActiveTransport::Gsm(t) => t.ensure_connected().await,
        }
    }

    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome {
        match self {
            ActiveTransport::Gsm(t) => t.send_data(packages).await,
        }
    }

    async fn control(&mut self, cmd: super::TransportControl, id: CorrelationId) -> ControlOutcome {
        match self {
            ActiveTransport::Gsm(t) => t.control(cmd, id).await,
        }
    }
}
