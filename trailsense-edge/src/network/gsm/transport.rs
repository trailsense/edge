extern crate alloc;
use alloc::vec::Vec;
use embassy_executor::Spawner;
use esp_hal::{Async, uart::Uart};
use log::{error, info};
use serde_json::to_string;

use crate::network::{
    TransportControl, UplinkTransport,
    config::{DEVICE_ID, ingest_url},
    gsm::{
        commands::{GsmError, GsmErrorKind},
        modem::GsmModem,
    },
    types::{ConnectionOutcome, ControlOutcome, PackageDto, SendDataOutcome},
};
use crate::orchestration::types::CorrelationId;
use crate::packages::package_store::PackageEntity;

pub struct GsmTransport {
    modem: GsmModem,
    auto_connect_enabled: bool,
}

impl GsmTransport {
    pub fn new(uart: Uart<'static, Async>, spawner: Spawner) -> Result<Self, GsmError> {
        let modem = GsmModem::new(uart, spawner).map_err(|e| {
            error!("Error initiating GSM modem: {:?}", e);
            e
        })?;

        Ok(GsmTransport {
            modem,
            auto_connect_enabled: true,
        })
    }
}

impl UplinkTransport for GsmTransport {
    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome {
        if !self.auto_connect_enabled {
            return SendDataOutcome::BackoffRequired;
        }

        let payload: Vec<PackageDto<'_>> = packages
            .iter()
            .map(|p| PackageDto::new(p.age_in_seconds, p.count, DEVICE_ID))
            .collect();

        let url = match ingest_url() {
            Ok(u) => u,
            Err(e) => {
                error!("Failed to generate URL: {}", e);
                return SendDataOutcome::FatalFailure;
            }
        };

        let body = match to_string(&payload) {
            Ok(v) => v,
            Err(e) => {
                error!("GSM payload serialization failed: {:?}", e);
                return SendDataOutcome::FatalFailure;
            }
        };
        info!(
            "GSM upload prepared: packages={}, body_len={}",
            payload.len(),
            body.len()
        );

        match self.modem.post_json(body.as_str(), url.as_str()).await {
            Ok(()) => SendDataOutcome::Success,
            Err(e) => {
                error!("GSM post_json failed: {:?}", e);
                match e.kind() {
                    GsmErrorKind::Transient => SendDataOutcome::RetryableFailure,
                    GsmErrorKind::Hard => SendDataOutcome::FatalFailure,
                }
            }
        }
    }

    async fn ensure_connected(&mut self) -> ConnectionOutcome {
        if !self.auto_connect_enabled {
            return ConnectionOutcome::Disconnected;
        }
        match self.modem.ensure_connected().await {
            Ok(()) => ConnectionOutcome::Connected,
            Err(e) => {
                error!("GSM ensure_connected failed: {:?}", e);
                match e.kind() {
                    GsmErrorKind::Transient => ConnectionOutcome::Disconnected,
                    GsmErrorKind::Hard => ConnectionOutcome::Failure,
                }
            }
        }
    }

    async fn control(&mut self, cmd: TransportControl, _id: CorrelationId) -> ControlOutcome {
        self.auto_connect_enabled = matches!(cmd, TransportControl::Enable);
        if !self.auto_connect_enabled {
            // match self.modem.disconnect().await {
            //     Ok(()) => ControlOutcome::Applied,
            //     Err(e) => {
            //         // Disabling transport is a policy-level switch; teardown is best effort.
            //         error!(
            //             "GSM disconnect failed during disable (best-effort): {:?}",
            //             e
            //         );
            //         ControlOutcome::Applied
            //     }
            // }
            ControlOutcome::Applied
        } else {
            ControlOutcome::Applied
        }
    }
}
