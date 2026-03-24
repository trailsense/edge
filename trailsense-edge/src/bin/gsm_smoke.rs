#![no_std]
#![no_main]

esp_bootloader_esp_idf::esp_app_desc!();

use atat::asynch::{AtatClient, Client};
use atat::digest::ParseError;
use atat::{
    AtatCmd, AtatIngress, DefaultDigester, Error, Ingress, InternalError, Parser, ResponseSlot,
    UrcChannel, UrcSubscription, atat_derive,
};
use core::fmt::Write;
use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config, Uart, UartRx};
use static_cell::StaticCell;

const BUF_SIZE: usize = 1024;
const URC_CAPACITY: usize = 1;
const URC_SUBSCRIBERS: usize = 1;

#[derive(atat_derive::AtatResp)]
struct NoResponse;

#[derive(atat_derive::AtatResp, Debug)]
struct IpResponse {
    ip: atat::heapless::String<32>,
}

#[derive(atat_derive::AtatCmd)]
#[at_cmd("+NETOPEN", NoResponse, timeout_ms = 5000)]
struct NetOpen;

#[derive(atat_derive::AtatCmd)]
#[at_cmd("+IPADDR", IpResponse, timeout_ms = 1000)]
struct GetIpAddr;

#[derive(atat_derive::AtatResp, Clone, Debug)]
struct HttpActionResult {
    method: u8,
    status: u16,
    len: u16,
}

#[derive(atat_derive::AtatUrc, Clone, Debug)]
enum Urc {
    #[at_urc(b"+HTTPACTION")]
    HttpAction(HttpActionResult),
}

struct RawAtCmd<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    cmd: &'a str,
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd for RawAtCmd<'_, MAX_LEN, TIMEOUT_MS> {
    type Response = NoResponse;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let bytes = self.cmd.as_bytes();
        let len = bytes.len();
        buf[..len].copy_from_slice(bytes);
        buf[len] = b'\r';
        buf[len + 1] = b'\n';
        len + 2
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, Error> {
        match resp {
            Ok(_) => Ok(NoResponse),
            Err(e) => Err(e.into()),
        }
    }
}

struct RawPayload<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    payload: &'a str,
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd for RawPayload<'_, MAX_LEN, TIMEOUT_MS> {
    type Response = NoResponse;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let bytes = self.payload.as_bytes();
        let len = bytes.len();
        buf[..len].copy_from_slice(bytes);
        len
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, Error> {
        match resp {
            Ok(_) => Ok(NoResponse),
            Err(e) => Err(e.into()),
        }
    }
}

struct RawAtReadCmd<'a, const MAX_LEN: usize, const TIMEOUT_MS: u32> {
    cmd: &'a str,
}

impl<const MAX_LEN: usize, const TIMEOUT_MS: u32> AtatCmd
    for RawAtReadCmd<'_, MAX_LEN, TIMEOUT_MS>
{
    type Response = atat::heapless::String<512>;
    const MAX_LEN: usize = MAX_LEN;
    const MAX_TIMEOUT_MS: u32 = TIMEOUT_MS;

    fn write(&self, buf: &mut [u8]) -> usize {
        let bytes = self.cmd.as_bytes();
        let len = bytes.len();
        buf[..len].copy_from_slice(bytes);
        buf[len] = b'\r';
        buf[len + 1] = b'\n';
        len + 2
    }

    fn parse(&self, resp: Result<&[u8], InternalError>) -> Result<Self::Response, Error> {
        let bytes = resp.map_err(Error::from)?;
        let s = core::str::from_utf8(bytes).map_err(|_| Error::Parse)?;
        atat::heapless::String::<512>::try_from(s).map_err(|_| Error::Parse)
    }
}

fn simcom_download_prompt(buf: &[u8]) -> Result<(u8, usize), ParseError> {
    for p in [b"\r\nDOWNLOAD\r\n".as_slice(), b"DOWNLOAD\r\n".as_slice()] {
        if buf.starts_with(p) {
            return Ok((b'>', p.len()));
        }
        if p.starts_with(buf) {
            return Err(ParseError::Incomplete);
        }
    }
    Err(ParseError::NoMatch)
}

struct HttpUrcParser;

impl Parser for HttpUrcParser {
    fn parse(buf: &[u8]) -> Result<(&[u8], usize), ParseError> {
        const URC: &[u8] = b"+HTTPACTION";
        let (line, offset) = if let Some(rest) = buf.strip_prefix(b"\r\n") {
            (rest, 2)
        } else {
            (buf, 0)
        };
        if !line.starts_with(URC) {
            if URC.starts_with(line) || b"\r\n+HTTPACTION".starts_with(buf) {
                return Err(ParseError::Incomplete);
            }
            return Err(ParseError::NoMatch);
        }
        if let Some(end) = line.windows(2).position(|w| w == b"\r\n") {
            return Ok((&line[..end], offset + end + 2));
        }
        Err(ParseError::Incomplete)
    }
}

async fn send_raw_cmd(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    cmd: &str,
) -> bool {
    let raw = RawAtCmd::<256, 20_000> { cmd };
    client.send(&raw).await.is_ok()
}

async fn send_raw_payload(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    payload: &str,
) -> bool {
    let raw = RawPayload::<256, 20_000> { payload };
    client.send(&raw).await.is_ok()
}

async fn send_raw_read_cmd(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    cmd: &str,
) -> Option<atat::heapless::String<512>> {
    let raw = RawAtReadCmd::<256, 20_000> { cmd };
    client.send(&raw).await.ok()
}

async fn post_https(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    urc_sub: &mut UrcSubscription<'static, Urc, URC_CAPACITY, URC_SUBSCRIBERS>,
) {
    let _ = send_raw_cmd(client, "AT+HTTPTERM").await;
    if !send_raw_cmd(client, "AT+HTTPINIT").await {
        return;
    }
    let _ = send_raw_cmd(client, "AT+CSSLCFG=\"sslversion\",0,4").await;
    let _ = send_raw_cmd(client, "AT+CSSLCFG=\"ignoreretc\",0,1").await;
    let _ = send_raw_cmd(client, "AT+CSSLCFG=\"enableSNI\",0,1").await;
    let _ = send_raw_cmd(client, "AT+HTTPPARA=\"SSLCFG\",0").await;
    if !send_raw_cmd(
        client,
        "AT+HTTPPARA=\"URL\",\"https://api.trailsense.daugt.com/ingest\"",
    )
    .await
    {
        return;
    }
    if !send_raw_cmd(client, "AT+HTTPPARA=\"CONTENT\",\"application/json\"").await {
        return;
    }

    let payload = "[{\"age_in_seconds\":69,\"count\":69,\"node_id\":\"71ec4873-944e-49c1-b7c4-4b856797715f\"}]";
    let mut cmd = atat::heapless::String::<64>::new();
    write!(cmd, "AT+HTTPDATA={},5000", payload.len()).ok();
    if !send_raw_cmd(client, &cmd).await {
        return;
    }
    if !send_raw_payload(client, payload).await {
        return;
    }

    let _ = send_raw_cmd(client, "AT+HTTPACTION=1").await;
    match embassy_time::with_timeout(
        embassy_time::Duration::from_secs(15),
        urc_sub.next_message_pure(),
    )
    .await
    {
        Ok(Urc::HttpAction(res)) => esp_println::println!(
            "HTTPACTION result: method={}, status={}, len={}",
            res.method,
            res.status,
            res.len
        ),
        Err(_) => esp_println::println!("HTTPACTION URC timeout"),
    }

    if let Some(body) = send_raw_read_cmd(client, "AT+HTTPREAD=0,512").await {
        esp_println::println!("HTTPREAD body: {}", body);
    }
    let _ = send_raw_cmd(client, "AT+HTTPTERM").await;
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let p = esp_hal::init(esp_hal::Config::default());
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0);

    let uart = Uart::new(p.UART2, Config::default().with_baudrate(115200))
        .unwrap()
        .with_tx(p.GPIO17)
        .with_rx(p.GPIO16)
        .into_async();
    let (reader, writer) = uart.split();

    static RES_SLOT: ResponseSlot<BUF_SIZE> = ResponseSlot::new();
    static INGRESS_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
    static CLIENT_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
    static URC_CHANNEL: UrcChannel<Urc, URC_CAPACITY, URC_SUBSCRIBERS> = UrcChannel::new();

    let mut urc_sub = URC_CHANNEL.subscribe().unwrap();
    let digester =
        DefaultDigester::<HttpUrcParser>::default().with_custom_prompt(simcom_download_prompt);
    let ingress = Ingress::new(
        digester,
        INGRESS_BUF.init([0; BUF_SIZE]),
        &RES_SLOT,
        &URC_CHANNEL,
    );
    let mut client = Client::new(
        writer,
        &RES_SLOT,
        CLIENT_BUF.init([0; BUF_SIZE]),
        atat::Config::default(),
    );
    spawner.spawn(ingress_task(ingress, reader)).unwrap();

    let _ = client.send(&NetOpen).await;
    loop {
        if let Ok(ip) = client.send(&GetIpAddr).await {
            esp_println::println!("Online! IP: {}", ip.ip);
            break;
        }
        embassy_time::Timer::after_secs(1).await;
    }

    loop {
        post_https(&mut client, &mut urc_sub).await;
        embassy_time::Timer::after_secs(20).await;
    }
}

#[embassy_executor::task]
async fn ingress_task(
    mut ingress: Ingress<
        'static,
        DefaultDigester<HttpUrcParser>,
        Urc,
        BUF_SIZE,
        URC_CAPACITY,
        URC_SUBSCRIBERS,
    >,
    mut reader: UartRx<'static, esp_hal::Async>,
) -> ! {
    ingress.read_from(&mut reader).await
}
