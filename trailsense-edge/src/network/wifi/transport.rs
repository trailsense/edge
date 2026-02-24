extern crate alloc;
use alloc::vec::Vec;

use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embassy_time::{Duration, Timer, WithTimeout};
use log::{error, info};
use reqwless::{
    client::{HttpClient, TlsConfig},
    request::RequestBuilder,
};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};

use crate::{
    network::{
        UplinkTransport,
        types::{ConnectionOutcome, PackageDto, SendDataOutcome},
    },
    packages::package_store::PackageEntity,
    wifi::{WifiCtx, tasks::WifiControlCmd, wait_for_connection},
};

const BASE_URL: &str = match option_env!("TRAILSENSE_API_URL") {
    Some(v) => v,
    None => "https://api.trailsense.daugt.com",
};

const DEVICE_ID: &str = match option_env!("TRAILSENSE_EDGE_ID") {
    Some(v) => v,
    None => "71ec4873-944e-49c1-b7c4-4b856797715f",
};

const REQUEST_BUILD_ATTEMPTS: u8 = 3;
const REQUEST_RETRY_DELAY: Duration = Duration::from_millis(750);

pub struct WifiTransportConfig {
    pub dns_reconnect_threshold: u8,
    pub dns_restart_threshold: u8,
}

impl Default for WifiTransportConfig {
    fn default() -> Self {
        Self {
            dns_reconnect_threshold: 2,
            dns_restart_threshold: 4,
        }
    }
}

pub struct WifiTransport {
    stack: Stack<'static>,
    tls_seed: u64,
    dns_reconnect_threshold: u8,
    consecutive_dns_failures: u8,
    dns_restart_threshold: u8,
    wifi_control_sender: Sender<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
    recovery_pending: bool,
}

impl WifiTransport {
    pub fn new(
        context: WifiCtx,
        config: WifiTransportConfig,
        wifi_control_sender: Sender<'static, CriticalSectionRawMutex, WifiControlCmd, 4>,
    ) -> Self {
        WifiTransport {
            stack: context.stack,
            tls_seed: context.tls_seed,
            consecutive_dns_failures: 0,
            recovery_pending: false,
            dns_reconnect_threshold: config.dns_reconnect_threshold,
            dns_restart_threshold: config.dns_restart_threshold,
            wifi_control_sender: wifi_control_sender,
        }
    }
}

impl UplinkTransport for WifiTransport {
    async fn ensure_connected(&mut self) -> ConnectionOutcome {
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

        let was_recovering = self.recovery_pending;

        if wait_for_connection(self.stack)
            .with_timeout(CONNECT_TIMEOUT)
            .await
            .is_err()
        {
            error!("WiFi connection timeout");
            return ConnectionOutcome::Disconnected;
        }

        if was_recovering {
            info!("WiFi recovery completed");
            self.recovery_pending = false;
        }

        ConnectionOutcome::Connected
    }
    async fn send_data(&mut self, packages: Vec<PackageEntity>) -> SendDataOutcome {
        if self.recovery_pending {
            return SendDataOutcome::RetryableFailure;
        }

        if self.consecutive_dns_failures >= self.dns_restart_threshold {
            self.recovery_pending = true;
            self.wifi_control_sender
                .send(WifiControlCmd::RestartController)
                .await;
            self.consecutive_dns_failures = 0;
            return SendDataOutcome::RetryableFailure;
        } else if self.consecutive_dns_failures >= self.dns_reconnect_threshold {
            self.recovery_pending = true;
            self.wifi_control_sender
                .send(WifiControlCmd::Reconnect)
                .await;
            return SendDataOutcome::RetryableFailure;
        }

        let mut rx_buffer = [0; 4096]; // TODO: Refactor to reuse static TLS RX/TX buffers instead of allocating new ones per call, to reduce memory usage on constrained devices.
        let mut tx_buffer = [0; 4096];
        let mut url = heapless::String::<128>::new();
        use core::fmt::Write;
        if let Err(e) = write!(&mut url, "{}/ingest", BASE_URL) {
            error!("Failed to generate URL: {}", e);
            self.consecutive_dns_failures = 0;
            return SendDataOutcome::FatalFailure;
        }

        let dns = DnsSocket::new(self.stack);
        let tcp_state = TcpClientState::<1, 4096, 4096>::new();
        let tcp = TcpClient::new(self.stack, &tcp_state);

        let tls = TlsConfig::new(
            self.tls_seed,
            &mut rx_buffer,
            &mut tx_buffer,
            reqwless::client::TlsVerify::None, // TODO: this should be replaced by "Certificate" later on, we need to define the final domain for that.
        );

        let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

        let mut buffer = [0u8; 4096];

        let payload: Vec<PackageDto<'_>> = packages
            .iter()
            .map(|p| PackageDto::new(p.age_in_seconds, p.count, DEVICE_ID))
            .inspect(|dto| info!("Package: {:?}", dto))
            .collect();

        let body = match serde_json::to_vec(&payload) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to serialize payload: {:?}", e);
                self.consecutive_dns_failures = 0;
                return SendDataOutcome::FatalFailure;
            }
        };

        let request_builder = 'request: loop {
            for attempt in 0..REQUEST_BUILD_ATTEMPTS {
                match client
                    .request(reqwless::request::Method::POST, url.as_str())
                    .await
                {
                    Ok(builder) => break 'request builder,
                    Err(e) => {
                        error!(
                            "Failed to build HTTP request: url='{}', attempt {}/{}, err={:?}",
                            url.as_str(),
                            attempt + 1,
                            REQUEST_BUILD_ATTEMPTS,
                            e
                        );
                        if matches!(e, reqwless::Error::Dns) {
                            self.consecutive_dns_failures += 1;
                            return SendDataOutcome::RetryableFailure;
                        }
                        if attempt + 1 < REQUEST_BUILD_ATTEMPTS {
                            Timer::after(REQUEST_RETRY_DELAY).await;
                        }
                    }
                }
            }
            self.consecutive_dns_failures = 0;
            return SendDataOutcome::FatalFailure;
        };

        let mut http_req = request_builder
            .content_type(reqwless::headers::ContentType::ApplicationJson)
            .body(body.as_slice());

        let response = match http_req.send(&mut buffer).await {
            Ok(r) => r,
            Err(e) => {
                error!(
                    "HTTP POST send failed: url='{}', payload_len={}, err={:?}",
                    url.as_str(),
                    body.len(),
                    e
                );
                if matches!(e, reqwless::Error::Dns) {
                    self.consecutive_dns_failures += 1;
                    return SendDataOutcome::RetryableFailure;
                }
                self.consecutive_dns_failures = 0;
                return SendDataOutcome::FatalFailure;
            }
        };

        let status = response.status;
        let body = match response.body().read_to_end().await {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "HTTP response read failed: url='{}', status={:?}, err={:?}",
                    url.as_str(),
                    status,
                    e
                );
                self.consecutive_dns_failures = 0;
                return SendDataOutcome::FatalFailure;
            }
        };

        let body_content = match core::str::from_utf8(body) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "HTTP response UTF-8 decode failed: url='{}', status={:?}, body_len={}, err={:?}",
                    url.as_str(),
                    status,
                    body.len(),
                    e
                );
                self.consecutive_dns_failures = 0;
                return SendDataOutcome::FatalFailure;
            }
        };

        if status.is_successful() {
            info!("Success ({:?}): {}", status, body_content);
            self.consecutive_dns_failures = 0;
            SendDataOutcome::Success
        } else {
            error!("Error ({:?}): {}", status, body_content);
            self.consecutive_dns_failures = 0;
            SendDataOutcome::FatalFailure
        }
    }
}
