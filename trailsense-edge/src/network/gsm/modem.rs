use atat::{
    AtatIngress, DefaultDigester, Error as AtatError, Ingress, Response, ResponseSlot, UrcChannel,
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
    helpers::{send_raw_cmd, send_raw_cmd_quick, send_raw_payload, send_raw_read_cmd},
};

pub struct GsmModem {
    client: Client<'static, esp_hal::uart::UartTx<'static, esp_hal::Async>, BUF_SIZE>,
    res_slot: &'static ResponseSlot<BUF_SIZE>,
    urc_sub: UrcSubscription<'static, Urc, URC_CAPACITY, URC_SUBSCRIBERS>,
    use_ssl_ignore_cert: bool,
    use_ssl_sni: bool,
    network_open_confirmed: bool,
}
pub const BUF_SIZE: usize = 1024;
const URC_CAPACITY: usize = 1;
const URC_SUBSCRIBERS: usize = 1;
const MAX_IP_RETRIES: usize = 5;
const IP_RETRY_DELAY: u64 = 1;
const MAX_CONNECT_RETRIES: usize = 3;
const CONNECT_RETRY_DELAY: u64 = 2;
const HTTP_STEP_SETTLE_MS: u64 = 200;
const HTTP_RECOVERY_SETTLE_MS: u64 = 800;
const HTTP_POST_ATTEMPTS: usize = 1;
const HTTP_ACTION_TIMEOUT_SECS: u64 = 30;
const HTTP_PAYLOAD_RESPONSE_TIMEOUT_SECS: u64 = 20;

impl GsmModem {
    pub fn new(uart: Uart<'static, Async>, spawner: Spawner) -> Self {
        let (reader, writer) = uart.split();

        static RES_SLOT: ResponseSlot<BUF_SIZE> = ResponseSlot::new();
        static INGRESS_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
        static CLIENT_BUF: StaticCell<[u8; BUF_SIZE]> = StaticCell::new();
        static URC_CHANNEL: UrcChannel<Urc, URC_CAPACITY, URC_SUBSCRIBERS> = UrcChannel::new();

        let urc_sub = URC_CHANNEL.subscribe().unwrap();
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
        spawner.spawn(ingress_task(ingress, reader)).unwrap();
        GsmModem {
            client,
            res_slot: &RES_SLOT,
            urc_sub,
            use_ssl_ignore_cert: true,
            use_ssl_sni: true,
            network_open_confirmed: false,
        }
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
                // Only trust the IP fast-path after this runtime has successfully confirmed NETOPEN.
                match self.client.send(&GetIpAddr).await {
                    Ok(resp) if !resp.ip.is_empty() && resp.ip != "0.0.0.0" => {
                        info!("Already connected; IP address: '{}'", resp.ip);
                        return Ok(());
                    }
                    Ok(resp) => {
                        info!("IP not ready before NETOPEN: '{}'", resp.ip);
                    }
                    Err(e) => {
                        // Some modem states return a generic error here; continue with NETOPEN path.
                        info!("IP check before NETOPEN returned: {:?}", e);
                        last_err = Some(GsmError::Atat(e));
                    }
                }
            } else {
                info!("Skipping IP fast-path until NETOPEN is confirmed in this runtime");
            }

            if let Err(e) = self.open_network().await {
                // NETOPEN can error if already open or in transitional modem states.
                // Keep trying to get a valid IP before failing this attempt.
                info!("NETOPEN returned: {:?}; continuing with IP wait", e);
                last_err = Some(e);
            }

            match self.wait_for_ip().await {
                Ok(()) => {
                    self.network_open_confirmed = true;
                    info!("Connected after NETOPEN + IP wait");
                    return Ok(());
                }
                Err(e) => {
                    last_err = Some(e);
                    if matches!(
                        last_err.as_ref().map(GsmError::kind),
                        Some(GsmErrorKind::Hard)
                    ) {
                        break;
                    }
                    embassy_time::Timer::after_secs(CONNECT_RETRY_DELAY).await;
                }
            }
        }

        if let Some(e) = last_err {
            return Err(e);
        }

        Err(GsmError::IpTimeout)
    }
    pub async fn post_json(&mut self, _payload: &str) -> Result<(), GsmError> {
        let mut last_err: Option<GsmError> = None;

        for attempt in 1..=HTTP_POST_ATTEMPTS {
            info!("GSM HTTP session start attempt={}", attempt);
            best_effort_http_reset(&mut self.client).await;

            match self.run_http_post_session().await {
                Ok(()) => {
                    best_effort_http_reset(&mut self.client).await;
                    info!("GSM HTTP upload completed");
                    return Ok(());
                }
                Err(e) => {
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
            // Can fail when HTTP stack is already down; keep going.
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
                    // If modem no longer responds to IP query cleanly after NETCLOSE,
                    // propagate last transport command error below.
                    info!("GSM IP check after disconnect returned: {:?}", e);
                    break;
                }
            }

            embassy_time::Timer::after_secs(DISCONNECT_IP_CHECK_DELAY).await;
        }

        // If NETCLOSE succeeded but IP verification stayed inconclusive, accept as disconnected.
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
    async fn run_http_post_session(&mut self) -> Result<(), GsmError> {
        send_http_step(&mut self.client, "HTTPINIT", "AT+HTTPINIT").await?;
        send_http_step(
            &mut self.client,
            "CSSLCFG sslversion",
            "AT+CSSLCFG=\"sslversion\",0,4",
        )
        .await?;
        self.send_optional_ssl_step(
            "CSSLCFG ignoreretc",
            "AT+CSSLCFG=\"ignoreretc\",0,1",
            OptionalSslStep::IgnoreCert,
        )
        .await?;
        self.send_optional_ssl_step(
            "CSSLCFG enableSNI",
            "AT+CSSLCFG=\"enableSNI\",0,1",
            OptionalSslStep::EnableSni,
        )
        .await?;
        send_http_step(
            &mut self.client,
            "HTTPPARA SSLCFG",
            "AT+HTTPPARA=\"SSLCFG\",0",
        )
        .await?;
        send_http_step(
            &mut self.client,
            "HTTPPARA URL",
            "AT+HTTPPARA=\"URL\",\"https://api.trailsense.daugt.com/ingest\"",
        )
        .await?;
        send_http_step(
            &mut self.client,
            "HTTPPARA CONTENT",
            "AT+HTTPPARA=\"CONTENT\",\"application/json\"",
        )
        .await?;

        // Temporary: fixed payload from the original validated modem script for parity.
        let payload = "[{\"age_in_seconds\":69,\"count\":69,\"node_id\":\"71ec4873-944e-49c1-b7c4-4b856797715f\"}]";
        info!("GSM HTTP upload start; payload_len={}", payload.len());

        let mut cmd = atat::heapless::String::<64>::new();
        if write!(cmd, "AT+HTTPDATA={},5000", payload.len()).is_err() {
            return Err(GsmError::CommandBuildFailed);
        }

        send_http_step(&mut self.client, "HTTPDATA", &cmd).await?;

        info!("GSM HTTP step: PAYLOAD");
        if let Err(e) = send_raw_payload(&mut self.client, payload).await {
            error!("GSM HTTP step failed: PAYLOAD: {:?}", e);
            return Err(e);
        }
        wait_for_payload_response(self.res_slot).await?;

        send_http_step(&mut self.client, "HTTPACTION=1", "AT+HTTPACTION=1").await?;

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
        embassy_time::Timer::after_millis(HTTP_STEP_SETTLE_MS).await;
        Ok(())
    }

    async fn send_optional_ssl_step(
        &mut self,
        step: &str,
        cmd: &str,
        optional_step: OptionalSslStep,
    ) -> Result<(), GsmError> {
        let enabled = match optional_step {
            OptionalSslStep::IgnoreCert => self.use_ssl_ignore_cert,
            OptionalSslStep::EnableSni => self.use_ssl_sni,
        };

        if !enabled {
            info!("GSM HTTP step skipped: {}", step);
            return Ok(());
        }

        match send_http_step(&mut self.client, step, cmd).await {
            Ok(()) => Ok(()),
            Err(e) => {
                match optional_step {
                    OptionalSslStep::IgnoreCert => self.use_ssl_ignore_cert = false,
                    OptionalSslStep::EnableSni => self.use_ssl_sni = false,
                }
                info!("Disabling optional GSM SSL step after failure: {}", step);
                Err(e)
            }
        }
    }
}

enum OptionalSslStep {
    IgnoreCert,
    EnableSni,
}

async fn send_http_step(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
    step: &str,
    cmd: &str,
) -> Result<(), GsmError> {
    info!("GSM HTTP step: {}", step);
    if let Err(e) = send_raw_cmd(client, cmd).await {
        error!("GSM HTTP step failed: {}: {:?}", step, e);
        return Err(e);
    }
    embassy_time::Timer::after_millis(HTTP_STEP_SETTLE_MS).await;
    Ok(())
}

async fn best_effort_http_reset(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, BUF_SIZE>,
) {
    match send_raw_cmd_quick(client, "AT").await {
        Ok(()) => info!("GSM modem sync step: AT"),
        Err(e) => info!("GSM modem sync step AT returned: {:?}", e),
    }
    embassy_time::Timer::after_millis(HTTP_STEP_SETTLE_MS).await;

    match send_raw_cmd_quick(client, "AT+HTTPTERM").await {
        Ok(()) => info!("GSM modem reset step: HTTPTERM"),
        Err(e) => info!("GSM modem reset step HTTPTERM returned: {:?}", e),
    }
    embassy_time::Timer::after_millis(HTTP_RECOVERY_SETTLE_MS).await;
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

async fn wait_for_payload_response(
    res_slot: &'static ResponseSlot<BUF_SIZE>,
) -> Result<(), GsmError> {
    info!("GSM HTTP step: wait PAYLOAD response");
    let guard = match embassy_time::with_timeout(
        embassy_time::Duration::from_secs(HTTP_PAYLOAD_RESPONSE_TIMEOUT_SECS),
        res_slot.get(),
    )
    .await
    {
        Ok(guard) => guard,
        Err(_) => {
            error!("GSM HTTP step failed: wait PAYLOAD response timeout");
            res_slot.reset();
            return Err(GsmError::Atat(AtatError::Timeout));
        }
    };

    let response = guard.borrow();
    match &*response {
        Response::Ok(bytes) if bytes.is_empty() => {
            info!("GSM HTTP step: PAYLOAD response OK");
        }
        Response::Ok(bytes) => {
            if let Ok(text) = core::str::from_utf8(bytes.as_slice()) {
                info!("GSM HTTP step: PAYLOAD response text: {}", text);
            } else {
                info!("GSM HTTP step: PAYLOAD response bytes_len={}", bytes.len());
            }
        }
        Response::TimeoutError => {
            error!("GSM HTTP step failed: PAYLOAD response timeout error");
            drop(response);
            res_slot.reset();
            return Err(GsmError::Atat(AtatError::Timeout));
        }
        Response::ReadError => {
            error!("GSM HTTP step failed: PAYLOAD response read error");
            drop(response);
            res_slot.reset();
            return Err(GsmError::Atat(AtatError::Read));
        }
        Response::WriteError => {
            error!("GSM HTTP step failed: PAYLOAD response write error");
            drop(response);
            res_slot.reset();
            return Err(GsmError::Atat(AtatError::Write));
        }
        Response::AbortedError => {
            error!("GSM HTTP step failed: PAYLOAD response aborted");
            drop(response);
            res_slot.reset();
            return Err(GsmError::Atat(AtatError::Aborted));
        }
        other => {
            info!("GSM HTTP step: PAYLOAD response other: {:?}", other);
        }
    }
    drop(response);
    res_slot.reset();
    Ok(())
}

fn simcom_download_prompt(buf: &[u8]) -> Result<(u8, usize), ParseError> {
    // In integrated runtime we can receive extra bytes/URCs ahead of the DOWNLOAD prompt.
    // Accept the prompt even when it is not at buffer start.
    for p in [b"\r\nDOWNLOAD\r\n".as_slice(), b"DOWNLOAD\r\n".as_slice()] {
        if let Some(pos) = buf.windows(p.len()).position(|w| w == p) {
            return Ok((b'>', pos + p.len()));
        }
        // If buffer tail is a prefix of the prompt, wait for more bytes.
        let max_tail = core::cmp::min(buf.len(), p.len().saturating_sub(1));
        for tail in 1..=max_tail {
            if buf[buf.len() - tail..] == p[..tail] {
                return Err(ParseError::Incomplete);
            }
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
