extern crate alloc;
use crate::{
    network::types::{ConnectionOutcome, SendDataOutcome},
    packages::package_store::PackageEntity,
};
use alloc::vec::Vec;
pub mod factory;
pub mod active_transport;
pub mod types;
pub mod uploader;
pub mod wifi;

#[allow(async_fn_in_trait)]
pub trait UplinkTransport {
    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome;
    async fn ensure_connected(&mut self) -> ConnectionOutcome;
}
