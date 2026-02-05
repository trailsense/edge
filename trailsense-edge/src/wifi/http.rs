extern crate alloc;
use alloc::vec::Vec;

use alloc::string::String;
use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use log::{error, info};
use reqwless::{
    client::{HttpClient, TlsConfig},
    request::RequestBuilder,
};

use crate::packages::package_store::PackageEntity;

const BASE_URL: &str = match option_env!("TRAILSENSE_BASE_URL") {
    Some(v) => v,
    None => "https://api.trailsense.daugt.com",
};

const DEVICE_ID: &str = match option_env!("TRAILSENSE_EDGE_ID") {
    Some(v) => v,
    None => "71ec4873-944e-49c1-b7c4-4b856797715f",
};

pub async fn send_data(stack: Stack<'_>, tls_seed: u64, packages: Vec<PackageEntity>) -> bool {
    let mut rx_buffer = [0; 4096]; // TODO: Refactor to reuse static TLS RX/TX buffers instead of allocating new ones per call, to reduce memory usage on constrained devices.
    let mut tx_buffer = [0; 4096];

    let mut url = heapless::String::<128>::new();
    use core::fmt::Write;
    write!(&mut url, "{}/ingest", BASE_URL).unwrap();

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

    let mut body = String::new();
    body.push('[');
    for (i, p) in packages.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        write!(
            &mut body,
            "{{\"age_in_seconds\":{},\"count\":{},\"node_id\":\"{}\"}}",
            p.age_in_seconds, p.count, DEVICE_ID
        )
        .unwrap();
    }
    body.push(']');

    let mut http_req = client
        .request(reqwless::request::Method::POST, url.as_str())
        .await
        .unwrap()
        .content_type(reqwless::headers::ContentType::ApplicationJson)
        .body(body.as_bytes());

    let response = http_req.send(&mut buffer).await.unwrap();
    let status = response.status;
    let body = response.body().read_to_end().await.unwrap();

    let Ok(body_content) = core::str::from_utf8(body) else {
        error!("Something went wrong when parsing the content");
        return false;
    };

    if status.is_successful() {
        info!("Success ({:?}): {}", status, body_content);
        return true;
    } else {
        error!("Error ({:?}): {}", status, body_content);
        return false;
    }
}
