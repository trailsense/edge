use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use esp_println::println;
use log::error;
use reqwless::{
    client::{HttpClient, TlsConfig},
    request::RequestBuilder,
};

pub async fn send_data(stack: Stack<'_>, tls_seed: u64) {
    let mut rx_buffer = [0; 4096]; // TODO: Change these to either let them be reused or static in future. Look at PR 19 comment by copilot.
    let mut tx_buffer = [0; 4096];

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
    let mut http_req = client
        .request(
            reqwless::request::Method::POST,
            "https://api.trailsense.daugt.com/ingest/1",
        )
        .await
        .unwrap()
        .content_type(reqwless::headers::ContentType::ApplicationJson)
        .body(br#"{"wifi":200,"bluetooth":10}"#.as_slice());

    let response = http_req.send(&mut buffer).await.unwrap();
    let status = response.status;
    let body = response.body().read_to_end().await.unwrap();

    let Ok(body_content) = core::str::from_utf8(body) else {
        error!("Something went wrong when parsing the content");
        return;
    };

    if status.is_successful() {
        println!("Success ({:?}): {}", status, body_content);
    } else {
        println!("Error ({:?}): {}", status, body_content);
    }
}
