use atat::{
    AtatIngress, DefaultDigester, Error as AtatError, Ingress, ResponseSlot, UrcChannel,
    UrcSubscription,
    asynch::{AtatClient, Client},
    digest::ParseError,
};
use embassy_executor::Spawner;
use esp_hal::{Async, uart::Uart, uart::UartRx};
use log::error;
use static_cell::StaticCell;

use crate::network::gsm::commands::{GetIpAddr, GsmError, HttpUrcParser, NetOpen, Urc};

pub struct GsmModem {
    client: Client<'static, esp_hal::uart::UartTx<'static, esp_hal::Async>, BUF_SIZE>,
    urc_sub: UrcSubscription<'static, Urc, URC_CAPACITY, URC_SUBSCRIBERS>,
}
const BUF_SIZE: usize = 1024;
const URC_CAPACITY: usize = 1;
const URC_SUBSCRIBERS: usize = 1;
const MAX_IP_RETRIES: usize = 5;
const IP_RETRY_DELAY: u64 = 1;

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
        GsmModem { client, urc_sub }
    }
    pub async fn open_network(&mut self) -> Result<(), GsmError> {
        self.client.send(&NetOpen).await?;
        Ok(())
    }
    pub async fn wait_for_ip(&mut self) -> Result<(), GsmError> {
        let mut last_err: Option<AtatError> = None;

        for _attempt in 0..MAX_IP_RETRIES {
            match self.client.send(&GetIpAddr).await {
                Ok(resp) => {
                    if resp.ip.is_empty() {
                        error!("The ip address was empty")
                    } else if resp.ip != "" || resp.ip != "0.0.0.0" {
                        error!(
                            "The ip address recieved is in an incorrect format: {}",
                            resp.ip
                        )
                    } else {
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
    pub async fn ensure_connected(&mut self) {
        let mut is_network_open = false;
        loop {
            if !is_network_open {
                if let Err(_) = self.open_network().await {
                    continue;
                } else {
                    is_network_open = true;
                }
            }

            if let Err(_) = self.wait_for_ip().await {
                continue;
            }

            break;
        }
    }
    pub async fn post_json(&mut self, _payload: &str) {}
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
