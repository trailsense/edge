extern crate alloc;
use alloc::vec::Vec;

#[cfg(feature = "uplink-wifi")]
use crate::network::wifi::transport::WifiTransport;
use crate::{network::UplinkTransport, packages::package_store::PackageEntity};

#[derive(serde::Serialize, Debug)]
pub struct PackageDto<'a> {
    age_in_seconds: u64,
    count: u32,
    node_id: &'a str,
}

impl<'a> PackageDto<'a> {
    pub fn new(age_in_seconds: u64, count: u32, node_id: &'a str) -> Self {
        PackageDto {
            age_in_seconds,
            count,
            node_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendDataOutcome {
    Success,
    RetryableFailure,
    FatalFailure,
}

pub enum ConnectionOutcome {
    Connected,
    Disconnected,
    Failure,
}

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
