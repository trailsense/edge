extern crate alloc;
use alloc::vec::Vec;

use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embassy_time::{Duration, Timer};
use log::{error, info};
use reqwless::{
    client::{HttpClient, TlsConfig},
    request::RequestBuilder,
};

#[derive(serde::Serialize, Debug)]
struct PackageDto<'a> {
    age_in_seconds: u64,
    count: u32,
    node_id: &'a str,
}

use crate::packages::package_store::PackageEntity;

const BASE_URL: &str = match option_env!("TRAILSENSE_BASE_URL") {
    Some(v) => v,
    None => "https://trailsense-core-app.3pbn53.uncld.dev",
};

const DEVICE_ID: &str = match option_env!("TRAILSENSE_EDGE_ID") {
    Some(v) => v,
    None => "71ec4873-944e-49c1-b7c4-4b856797715f",
};

const REQUEST_BUILD_ATTEMPTS: u8 = 3;
const REQUEST_RETRY_DELAY: Duration = Duration::from_millis(750);

pub async fn send_data(stack: Stack<'_>, tls_seed: u64, packages: Vec<PackageEntity>) -> bool {
    let mut rx_buffer = [0; 4096]; // TODO: Refactor to reuse static TLS RX/TX buffers instead of allocating new ones per call, to reduce memory usage on constrained devices.
    let mut tx_buffer = [0; 4096];

    let mut url = heapless::String::<128>::new();
    use core::fmt::Write;
    if let Err(e) = write!(&mut url, "{}/ingest", BASE_URL) {
        error!("Failed to generate URL: {}", e);
        return false;
    }

    let dns = DnsSocket::new(stack);
    let tcp_state = TcpClientState::<1, 4096, 4096>::new();
    let tcp = TcpClient::new(stack, &tcp_state);

    let tls = TlsConfig::new(
        tls_seed,
        &mut rx_buffer,
        &mut tx_buffer,
        reqwless::client::TlsVerify::None, // TODO: this should be replaced by "Certificate" later on, we need to define the final domain for that.
    );

    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

    let mut buffer = [0u8; 4096];

    let payload: Vec<PackageDto<'_>> = packages
        .iter()
        .map(|p| PackageDto {
            age_in_seconds: p.age_in_seconds,
            count: p.count,
            node_id: DEVICE_ID,
        })
        .inspect(|dto| info!("Package: {:?}", dto))
        .collect();

    let body = match serde_json::to_vec(&payload) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to serialize payload: {:?}", e);
            return false;
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
                    if attempt + 1 < REQUEST_BUILD_ATTEMPTS {
                        Timer::after(REQUEST_RETRY_DELAY).await;
                    }
                }
            }
        }
        return false;
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
            return false;
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
            return false;
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
            return false;
        }
    };

    if status.is_successful() {
        info!("Success ({:?}): {}", status, body_content);
        true
    } else {
        error!("Error ({:?}): {}", status, body_content);
        false
    }
}
