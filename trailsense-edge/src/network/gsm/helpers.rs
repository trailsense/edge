use atat::asynch::{AtatClient, Client};

use crate::network::gsm::commands::{GsmError, RawAtCmd, RawAtReadCmd, RawPayload};

const RAW_CMD_TIMEOUT_MS: u32 = 30_000;
const QUICK_RAW_CMD_TIMEOUT_MS: u32 = 2_000;
const RAW_PAYLOAD_TIMEOUT_MS: u32 = 45_000;

async fn send_raw_cmd_inner<const INGRESS_BUF_SIZE: usize, const TIMEOUT_MS: u32>(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<(), GsmError> {
    let needed = cmd.len() + 2; // CRLF
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawAtCmd::<256, TIMEOUT_MS>::new(cmd);
    client.send(&raw).await.map_err(GsmError::from)?;
    Ok(())
}

pub async fn send_raw_cmd<const INGRESS_BUF_SIZE: usize>(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<(), GsmError> {
    send_raw_cmd_inner::<INGRESS_BUF_SIZE, RAW_CMD_TIMEOUT_MS>(client, cmd).await
}

pub async fn send_raw_cmd_quick<const INGRESS_BUF_SIZE: usize>(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<(), GsmError> {
    send_raw_cmd_inner::<INGRESS_BUF_SIZE, QUICK_RAW_CMD_TIMEOUT_MS>(client, cmd).await
}

pub async fn send_raw_payload<const INGRESS_BUF_SIZE: usize>(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    payload: &str,
) -> Result<(), GsmError> {
    let needed = payload.len();
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawPayload::<256, RAW_PAYLOAD_TIMEOUT_MS>::new(payload);
    client.send(&raw).await.map_err(GsmError::from)?;
    Ok(())
}

pub async fn send_raw_read_cmd<const INGRESS_BUF_SIZE: usize>(
    client: &mut Client<'_, esp_hal::uart::UartTx<'_, esp_hal::Async>, INGRESS_BUF_SIZE>,
    cmd: &str,
) -> Result<atat::heapless::String<512>, GsmError> {
    let needed = cmd.len() + 2; // CRLF
    if needed > INGRESS_BUF_SIZE {
        return Err(GsmError::BufferTooSmall {
            needed,
            available: INGRESS_BUF_SIZE,
        });
    }
    let raw = RawAtReadCmd::<256, RAW_CMD_TIMEOUT_MS>::new(cmd);
    client.send(&raw).await.map_err(GsmError::from)
}
