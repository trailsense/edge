extern crate alloc;
use crate::network::types::ControlOutcome;
use crate::{
    network::types::{ConnectionOutcome, SendDataOutcome},
    orchestration::types::CorrelationId,
    packages::package_store::PackageEntity,
};
use alloc::vec::Vec;
pub mod active_transport;
pub mod config;
pub mod factory;
#[cfg(feature = "uplink-gsm")]
pub mod gsm;
pub mod types;
pub mod uploader;
pub mod wifi;

pub enum TransportControl {
    Enable,
    Disable,
}

#[allow(async_fn_in_trait)]
pub trait UplinkTransport {
    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome;
    async fn ensure_connected(&mut self) -> ConnectionOutcome;
    async fn control(&mut self, cmd: TransportControl, id: CorrelationId) -> ControlOutcome;
}
