use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use esp_println::println;
use reqwless::client::{HttpClient, TlsConfig};

pub async fn access_website(stack: Stack<'_>, tls_seed: u64) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let dns = DnsSocket::new(stack);
    let tcp_state = TcpClientState::<1, 4096, 4096>::new();
    let tcp = TcpClient::new(stack, &tcp_state);

    let tls = TlsConfig::new(
        tls_seed,
        &mut rx_buffer,
        &mut tx_buffer,
        reqwless::client::TlsVerify::None,
    );

    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

    let mut buffer = [0u8; 4096];
    let mut http_req = client
        .request(
            reqwless::request::Method::GET,
            "https://jsonplaceholder.typicode.com/posts/1",
        )
        .await
        .unwrap();

    let response = http_req.send(&mut buffer).await.unwrap();
    let body = response.body().read_to_end().await.unwrap();
    println!("{}", core::str::from_utf8(body).unwrap());
}
