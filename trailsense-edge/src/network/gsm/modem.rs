use atat::{
    AtatIngress, DefaultDigester, Error as AtatError, Ingress, ResponseSlot, UrcChannel,
    UrcSubscription,
    asynch::{AtatClient, Client},
    digest::ParseError,
};
use core::fmt::Write;
use embassy_executor::Spawner;
use esp_hal::{Async, uart::Uart, uart::UartRx};
use log::{error, info};
use static_cell::StaticCell;

use crate::network::gsm::{
    commands::{GetIpAddr, GsmError, GsmErrorKind, HttpUrcParser, NetOpen, Urc},
    helpers::{send_raw_cmd, send_raw_payload, send_raw_read_cmd},
};

pub struct GsmModem {
    client: Client<'static, esp_hal::uart::UartTx<'static, esp_hal::Async>, BUF_SIZE>,
    urc_sub: UrcSubscription<'static, Urc, URC_CAPACITY, URC_SUBSCRIBERS>,
    network_open_confirmed: bool,
}
pub const BUF_SIZE: usize = 1024;
const URC_CAPACITY: usize = 1;
const URC_SUBSCRIBERS: usize = 1;
const MAX_IP_RETRIES: usize = 5;
const IP_RETRY_DELAY: u64 = 1;
const MAX_CONNECT_RETRIES: usize = 3;
const CONNECT_RETRY_DELAY: u64 = 2;
const HTTP_POST_ATTEMPTS: usize = 1;
const HTTP_ACTION_TIMEOUT_SECS: u64 = 15;
const HTTP_DATA_INPUT_TIMEOUT_MS: u32 = 5_000;

impl GsmModem {
    pub fn new(uart: Uart<'static, Async>, spawner: Spawner) -> Result<Self, GsmError> {
        let (reader, writer) = uart.split();

        static RES_SLOT: ResponseSlot<BUF_SIZE> = ResponseSlot::new();
        static INGRESS_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
        static CLIENT_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
        static URC_CHANNEL: UrcChannel<Urc, URC_CAPACITY, URC_SUBSCRIBERS> = UrcChannel::new();

        let urc_sub = match URC_CHANNEL.subscribe() {
            Ok(u) => u,
            Err(e) => {
                error!("GSM Init Error:{e:?}");
                return Err(GsmError::GsmInitError);
            }
        };

        let digester =
            DefaultDigester::<HttpUrcParser>::default().with_custom_prompt(simcom_download_prompt);
        let ingress = Ingress::new(
            digester,
            INGRESS_BUF.init([0; BUF_SIZE]),
            &RES_SLOT,
            &URC_CHANNEL,
        );
        let client = Client::new(
            writer,
            &RES_SLOT,
            CLIENT_BUF.init([0; BUF_SIZE]),
            atat::Config::default(),
        );
        if let Err(e) = spawner.spawn(ingress_task(ingress, reader)) {
            error!("GSM Init Error:{e}");
            return Err(GsmError::GsmInitError);
        }
        Ok(GsmModem {
            client,
            urc_sub,
            network_open_confirmed: false,
        })
    }
    pub async fn open_network(&mut self) -> Result<(), GsmError> {
        self.client.send(&NetOpen).await?;
        self.network_open_confirmed = true;
        Ok(())
    }
    pub async fn wait_for_ip(&mut self) -> Result<(), GsmError> {
        let mut last_err: Option<AtatError> = None;

        for _attempt in 0..MAX_IP_RETRIES {
            match self.client.send(&GetIpAddr).await {
                Ok(resp) => {
                    if resp.ip.is_empty() || resp.ip == "0.0.0.0" {
                        error!("IP not ready yet: '{}'", resp.ip);
                    } else {
                        info!("Assigned IP address: '{}'", resp.ip);
                        return Ok(());
                    }
                }
                Err(e) => last_err = Some(e),
            }

            embassy_time::Timer::after_secs(IP_RETRY_DELAY).await;
        }

        if let Some(e) = last_err {
            return Err(GsmError::Atat(e));
        }

        Err(GsmError::IpTimeout)
    }
    pub async fn ensure_connected(&mut self) -> Result<(), GsmError> {
        let mut last_err: Option<GsmError> = None;

        for _attempt in 0..MAX_CONNECT_RETRIES {
            if self.network_open_confirmed {
                match self.client.send(&GetIpAddr).await {
                    Ok(resp) if !resp.ip.is_empty() && resp.ip != "0.0.0.0" => {
                        info!("Already connected; IP address: '{}'", resp.ip);
                        return Ok(());
                    }
                    Ok(resp) => {
                        info!("IP not ready before NETOPEN: '{}'", resp.ip);
                    }
                    Err(e) => {
                        info!("IP check before NETOPEN returned: {:?}", e);
                    }
                }
            } else {
                info!("Skipping IP fast-path until NETOPEN is confirmed in this runtime");
            }

            if let Err(e) = self.open_network().await {
                info!("NETOPEN returned: {:?}; continuing with IP wait", e);
            }

            match self.wait_for_ip().await {
                Ok(()) => {
                    self.network_open_confirmed = true;
                    info!("Connected after NETOPEN + IP wait");
                    return Ok(());
                }
                Err(e) => {
                    if e.kind() == GsmErrorKind::Hard {
                        last_err = Some(e);
                        break;
                    }
                    last_err = Some(e);
                    embassy_time::Timer::after_secs(CONNECT_RETRY_DELAY).await;
                }
            }
        }

        if let Some(e) = last_err {
            return Err(e);
        }

        Err(GsmError::IpTimeout)
    }
    pub async fn post_json(&mut self, payload: &str) -> Result<(), GsmError> {
        let mut last_err: Option<GsmError> = None;

        for attempt in 1..=HTTP_POST_ATTEMPTS {
            info!("GSM HTTP session start attempt={}", attempt);

            match self.run_http_post_session(payload).await {
                Ok(()) => {
                    info!("GSM HTTP upload completed");
                    return Ok(());
                }
                Err(e) => {
                    self.network_open_confirmed = false;
                    if let Err(disconnect_err) = self.disconnect().await {
                        info!(
                            "GSM disconnect after HTTP failure returned: {:?}",
                            disconnect_err
                        );
                    }
                    error!("GSM HTTP session attempt={} failed: {:?}", attempt, e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or(GsmError::HttpActionTimeout))
    }

    pub async fn disconnect(&mut self) -> Result<(), GsmError> {
        const DISCONNECT_IP_CHECK_RETRIES: usize = 3;
        const DISCONNECT_IP_CHECK_DELAY: u64 = 1;

        let http_term_res = send_raw_cmd(&mut self.client, "AT+HTTPTERM").await;
        if let Err(e) = &http_term_res {
            info!("GSM HTTPTERM returned: {:?}", e);
        }

        let net_close_res = send_raw_cmd(&mut self.client, "AT+NETCLOSE").await;
        if let Err(e) = &net_close_res {
            error!("GSM NETCLOSE failed: {:?}", e);
        } else {
            self.network_open_confirmed = false;
        }

        for _ in 0..DISCONNECT_IP_CHECK_RETRIES {
            match self.client.send(&GetIpAddr).await {
                Ok(resp) if resp.ip.is_empty() || resp.ip == "0.0.0.0" => {
                    info!("GSM disconnect verified; ip='{}'", resp.ip);
                    return Ok(());
                }
                Ok(resp) => {
                    info!("GSM still attached during disconnect; ip='{}'", resp.ip);
                }
                Err(e) => {
                    info!("GSM IP check after disconnect returned: {:?}", e);
                    break;
                }
            }

            embassy_time::Timer::after_secs(DISCONNECT_IP_CHECK_DELAY).await;
        }

        if net_close_res.is_ok() {
            return Ok(());
        }

        if let Err(e) = net_close_res {
            return Err(e);
        }
        if let Err(e) = http_term_res {
            return Err(e);
        }
        Ok(())
    }
}

impl GsmModem {
    async fn run_http_post_session(&mut self, payload: &str) -> Result<(), GsmError> {
        info!("GSM HTTP step: HTTPTERM");
        let _ = send_raw_cmd(&mut self.client, "AT+HTTPTERM").await;

        run_http_step(&mut self.client, "HTTPINIT", "AT+HTTPINIT").await?;
        run_http_step(
            &mut self.client,
            "CSSLCFG sslversion",
            "AT+CSSLCFG=\"sslversion\",0,4",
        )
        .await?;
        run_http_step(
            &mut self.client,
            "CSSLCFG ignorelocaltime",
            "AT+CSSLCFG=\"ignorelocaltime\",0,1",
        )
        .await?;
        run_http_step(
            &mut self.client,
            "CSSLCFG enableSNI",
            "AT+CSSLCFG=\"enableSNI\",0,1",
        )
        .await?;
        run_http_step(
            &mut self.client,
            "HTTPPARA SSLCFG",
            "AT+HTTPPARA=\"SSLCFG\",0",
        )
        .await?;
        run_http_step(
            &mut self.client,
            "HTTPPARA URL",
            "AT+HTTPPARA=\"URL\",\"https://api.trailsense.daugt.com/ingest\"",
        )
        .await?;
        run_http_step(
            &mut self.client,
            "HTTPPARA CONTENT",
            "AT+HTTPPARA=\"CONTENT\",\"application/json\"",
        )
        .await?;

        info!("GSM HTTP upload start; payload_len={}", payload.len());

        let mut cmd = atat::heapless::String::<64>::new();
        if write!(
            cmd,
            "AT+HTTPDATA={},{}",
            payload.len(),
            HTTP_DATA_INPUT_TIMEOUT_MS
        )
        .is_err()
        {
            return Err(GsmError::CommandBuildFailed);
        }

        run_http_step(&mut self.client, "HTTPDATA", cmd.as_str()).await?;

        info!("GSM HTTP step: PAYLOAD");
        if let Err(e) = send_raw_payload(&mut self.client, payload).await {
            error!("GSM HTTP step failed: PAYLOAD: {:?}", e);
            return Err(e);
        }

        run_http_step(&mut self.client, "HTTPACTION=1", "AT+HTTPACTION=1").await?;

        info!("GSM HTTP step: wait HTTPACTION URC");
        let action = match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(HTTP_ACTION_TIMEOUT_SECS),
            self.urc_sub.next_message_pure(),
        )
        .await
        {
            Ok(v) => v,
            Err(_) => {
                error!("GSM HTTP step failed: wait HTTPACTION URC timeout");
                log_http_action_timeout_diagnostics(&mut self.client).await;
                self.network_open_confirmed = false;
                return Err(GsmError::HttpActionTimeout);
            }
        };

        let status = match action {
            Urc::HttpAction(res) => {
                info!(
                    "HTTPACTION result: method={}, status={}, len={}",
                    res.method, res.status, res.len
                );
                res.status
            }
        };

        if !(200..300).contains(&status) {
            return Err(GsmError::HttpStatus(status));
        }

        info!("GSM HTTP step: HTTPREAD");
        if let Ok(body) = send_raw_read_cmd(&mut self.client, "AT+HTTPREAD=0,512").await {
            info!("HTTPREAD body: {}", body);
        }
        let _ = send_raw_cmd(&mut self.client, "AT+HTTPTERM").await;
        Ok(())
    }
}

async fn run_http_step(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    label: &str,
    cmd: &str,
) -> Result<(), GsmError> {
    info!("GSM HTTP step: {}", label);
    if let Err(e) = send_raw_cmd(client, cmd).await {
        error!("GSM HTTP step failed: {}: {:?}", label, e);
        return Err(e);
    }
    Ok(())
}

async fn log_http_action_timeout_diagnostics(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
) {
    info!("GSM HTTP timeout diagnostics: HTTPREAD");
    match send_raw_read_cmd(client, "AT+HTTPREAD=0,512").await {
        Ok(body) => info!("GSM HTTP timeout diagnostics HTTPREAD body: {}", body),
        Err(e) => info!("GSM HTTP timeout diagnostics HTTPREAD failed: {:?}", e),
    }

    info!("GSM HTTP timeout diagnostics: HTTPTERM");
    match send_raw_cmd(client, "AT+HTTPTERM").await {
        Ok(()) => info!("GSM HTTP timeout diagnostics HTTPTERM ok"),
        Err(e) => info!("GSM HTTP timeout diagnostics HTTPTERM failed: {:?}", e),
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
