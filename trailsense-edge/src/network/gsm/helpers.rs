use atat::asynch::{AtatClient, Client};
use esp_hal::uart::UartTx;

use crate::network::gsm::commands::{GsmError, RawAtCmd, RawAtReadCmd, RawPayload};

pub const DEFAULT_RAW_CMD_TIMEOUT_MS: u32 = 20_000;
pub const DEFAULT_RAW_PAYLOAD_TIMEOUT_MS: u32 = 20_000;

// INGRESS_BUF_SIZE was used as our maximum size of messages. To increase the size, we should test it and see, but only if needed
async fn send_raw_cmd_inner<const INGRESS_BUF_SIZE: usize, const TIMEOUT_MS: u32>(
    client: &mut Client<'_, UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<(), GsmError> {
    let needed = cmd.len() + 2; // CRLF
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawAtCmd::<INGRESS_BUF_SIZE, TIMEOUT_MS>::new(cmd);
    client.send(&raw).await.map_err(GsmError::from)?;
    Ok(())
}

pub async fn send_raw_cmd<const INGRESS_BUF_SIZE: usize, const TIMEOUT_MS: u32>(
    client: &mut Client<'_, UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<(), GsmError> {
    send_raw_cmd_inner::<INGRESS_BUF_SIZE, TIMEOUT_MS>(client, cmd).await
}

pub async fn send_raw_payload<const INGRESS_BUF_SIZE: usize>(
    client: &mut Client<'_, UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    payload: &str,
) -> Result<(), GsmError> {
    let needed = payload.len();
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawPayload::<INGRESS_BUF_SIZE, DEFAULT_RAW_PAYLOAD_TIMEOUT_MS>::new(payload);
    client.send(&raw).await.map_err(GsmError::from)?;
    Ok(())
}

pub async fn send_raw_read_cmd<const INGRESS_BUF_SIZE: usize, const TIMEOUT_MS: u32>(
    client: &mut Client<'_, UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<atat::heapless::String<512>, GsmError> {
    send_raw_read_cmd_inner::<INGRESS_BUF_SIZE, TIMEOUT_MS>(client, cmd).await
}

async fn send_raw_read_cmd_inner<const INGRESS_BUF_SIZE: usize, const TIMEOUT_MS: u32>(
    client: &mut Client<'_, UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<atat::heapless::String<512>, GsmError> {
    let needed = cmd.len() + 2; // CRLF
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawAtReadCmd::<INGRESS_BUF_SIZE, TIMEOUT_MS>::new(cmd);
    client.send(&raw).await.map_err(GsmError::from)
}
