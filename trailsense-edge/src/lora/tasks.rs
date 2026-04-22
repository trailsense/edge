use core::str::from_utf8;

use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Delay, Duration, Timer};
use esp_hal::{
    Async,
    gpio::{Input, Output},
    spi::master::Spi,
};
use log::info;
use lora_phy::{
    LoRa, RxMode,
    iv::GenericSx126xInterfaceVariant,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx126x::{Sx126x, Sx1262},
};

use crate::lora::types::LORA_FREQUENCY_IN_HZ;

#[embassy_executor::task]
pub async fn recieve_lora_packets(
    mut lora: LoRa<
        Sx126x<
            SpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, Output<'static>>,
            GenericSx126xInterfaceVariant<Output<'static>, Input<'static>>,
            Sx1262,
        >,
        Delay,
    >,
) {
    let modulation_params = {
        match lora.create_modulation_params(
            SpreadingFactor::_7,
            Bandwidth::_125KHz,
            CodingRate::_4_5,
            LORA_FREQUENCY_IN_HZ,
        ) {
            Ok(mp) => mp,
            Err(err) => {
                info!("Radio error = {:?}", err);
                return;
            }
        }
    };

    let mut tx_packet_params = {
        match lora.create_tx_packet_params(8, false, true, false, &modulation_params) {
            Ok(pp) => pp,
            Err(err) => {
                info!("Radio error = {:?}", err);
                return;
            }
        }
    };

    let mut rx_packet_params = {
        match lora.create_rx_packet_params(8, false, 255, true, false, &modulation_params) {
            Ok(pp) => pp,
            Err(err) => {
                info!("Radio error = {:?}", err);
                return;
            }
        }
    };

    info!("LoRa ready on node");
    let mut rx_buffer = [0u8; 255];

    loop {
        match lora
            .prepare_for_rx(RxMode::Continuous, &modulation_params, &rx_packet_params)
            .await
        {
            Ok(()) => {}
            Err(err) => {
                info!("Radio error = {:?}", err);
                return;
            }
        }

        match lora.rx(&rx_packet_params, &mut rx_buffer).await {
            Ok((len, received_packet_status)) => {
                let payload = &rx_buffer[..len as usize];
                let as_text = from_utf8(payload).unwrap_or("<binary>");
                info!(
                    "RX: {} (RSSI {}, SNR {})",
                    as_text, received_packet_status.rssi, received_packet_status.snr
                );
                let mut tx_buf = [0u8; 40];
                let msg = if let Ok(text) = format_to_buf(&mut tx_buf, b"ACK", 16) {
                    text
                } else {
                    info!("Formatting error");
                    b"ERR"
                };

                match lora
                    .prepare_for_tx(&modulation_params, &mut tx_packet_params, 20, msg)
                    .await
                {
                    Ok(()) => {}
                    Err(err) => {
                        info!("Transmission Error = {:?}", err);
                        return;
                    }
                }

                match lora.tx().await {
                    Ok(()) => {
                        info!("Sent Msg over lora");
                    }
                    Err(err) => {
                        info!("Transmission Error = {:?}", err);
                        return;
                    }
                };
            }
            Err(err) => {
                info!("Receiving error {:?}", err);
                Timer::after(Duration::from_millis(200)).await;
            }
        }
    }
}

// TODO: Not written by me. Should be read through and rechecked
fn format_to_buf<'a>(buf: &'a mut [u8], content: &[u8], node_id: u32) -> Result<&'a [u8], ()> {
    let mut buf_index = 0;
    let mut buffer = [0u8; 10];
    let node_id = u32_to_ascii(node_id, &mut buffer);
    buf_index = copy_slice(buf, buf_index, node_id)?;
    buf_index = copy_slice(buf, buf_index, content)?;
    buf_index = copy_slice(buf, buf_index, b" #")?;

    Ok(&buf[..buf_index])
}

fn copy_slice(dst: &mut [u8], start: usize, src: &[u8]) -> Result<usize, ()> {
    let end = start.checked_add(src.len()).ok_or(())?;
    if end > dst.len() {
        return Err(());
    }
    dst[start..end].copy_from_slice(src);
    Ok(end)
}

fn u32_to_ascii<'a>(mut n: u32, out: &'a mut [u8; 10]) -> &'a [u8] {
    if n == 0 {
        out[0] = b'0';
        return &out[..1];
    }

    let mut i = out.len();
    while n > 0 {
        i -= 1;
        out[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &out[i..]
}
