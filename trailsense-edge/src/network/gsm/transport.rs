extern crate alloc;
use alloc::vec::Vec;
use embassy_executor::Spawner;
use esp_hal::{Async, uart::Uart};

use crate::network::{
    TransportControl, UplinkTransport,
    gsm::modem::GsmModem,
    types::{ConnectionOutcome, SendDataOutcome},
};
use crate::orchestration::types::CorrelationId;
use crate::packages::package_store::PackageEntity;

pub struct GsmTransport {
    modem: GsmModem,
    auto_connect_enabled: bool,
}

impl GsmTransport {
    pub fn new(uart: Uart<'static, Async>, spawner: Spawner) -> Self {
        GsmTransport {
            modem: GsmModem::new(uart, spawner),
            auto_connect_enabled: true,
        }
    }
}

impl UplinkTransport for GsmTransport {
    async fn send_data(&mut self, _packages: Vec<PackageEntity>) -> SendDataOutcome {
        if !self.auto_connect_enabled {
            return SendDataOutcome::BackoffRequired;
        }
        // Payload integration comes next; keep the transport surface stable first.
        self.modem.post_json("[]").await;
        SendDataOutcome::RetryableFailure
    }

    async fn ensure_connected(&mut self) -> ConnectionOutcome {
        if !self.auto_connect_enabled {
            return ConnectionOutcome::Disconnected;
        }
        self.modem.ensure_connected().await;
        ConnectionOutcome::Connected
    }

    async fn control(&mut self, cmd: TransportControl, _id: CorrelationId) {
        self.auto_connect_enabled = matches!(cmd, TransportControl::Enable);
    }
}
