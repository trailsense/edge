#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    peripherals::Peripherals,
    timer::timg::TimerGroup,
    uart::{Config, Uart},
    Async,
};
use log::{error, info};
use trailsense_edge::{
    network::{
        TransportControl, UplinkTransport,
        gsm::{UART_BAUDRATE, transport::GsmTransport},
        types::{ConnectionOutcome, ControlOutcome, SendDataOutcome},
    },
    orchestration::types::CorrelationId,
    packages::package_store::PackageEntity,
};

esp_bootloader_esp_idf::esp_app_desc!();

const CONNECT_ATTEMPTS: u8 = 5;
const CONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = init_hardware();

    esp_println::logger::init_logger_from_env();

    let uart = match Uart::new(
        peripherals.UART2,
        Config::default().with_baudrate(UART_BAUDRATE),
    ) {
        Ok(u) => u,
        Err(_) => {
            error!("DEVICE_TEST_FAIL modem_lifecycle_serial reason=uart_init_failed");
            loop {
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
    .with_tx(peripherals.GPIO17)
    .with_rx(peripherals.GPIO16)
    .into_async();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("DEVICE_TEST_START modem_lifecycle_serial");

    match run_modem_lifecycle(uart, spawner).await {
        Ok(()) => info!("DEVICE_TEST_PASS modem_lifecycle_serial"),
        Err(reason) => error!("DEVICE_TEST_FAIL modem_lifecycle_serial reason={}", reason),
    }

    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}

async fn run_modem_lifecycle(
    uart: Uart<'static, Async>,
    spawner: Spawner,
) -> Result<(), &'static str> {
    let mut transport =
        GsmTransport::new(uart, spawner).map_err(|_| "gsm_transport_init_failed")?;

    if !matches!(
        transport
            .control(TransportControl::Enable, CorrelationId(1))
            .await,
        ControlOutcome::Applied
    ) {
        return Err("transport_enable_failed");
    }

    if !wait_until_connected(&mut transport, CONNECT_ATTEMPTS).await {
        return Err("connect_timeout");
    }

    let upload_outcome = transport.send_data(vec![PackageEntity::new(1)]).await;
    if !matches!(
        upload_outcome,
        SendDataOutcome::Success | SendDataOutcome::RetryableFailure
    ) {
        return Err("upload_hard_failure");
    }

    if !matches!(
        transport
            .control(TransportControl::Disable, CorrelationId(2))
            .await,
        ControlOutcome::Applied
    ) {
        return Err("transport_disable_failed");
    }

    if !matches!(transport.ensure_connected().await, ConnectionOutcome::Disconnected) {
        return Err("transport_disable_not_effective");
    }

    if !matches!(
        transport
            .control(TransportControl::Enable, CorrelationId(3))
            .await,
        ControlOutcome::Applied
    ) {
        return Err("transport_reenable_failed");
    }

    if !wait_until_connected(&mut transport, CONNECT_ATTEMPTS).await {
        return Err("reconnect_timeout");
    }

    Ok(())
}

async fn wait_until_connected(transport: &mut GsmTransport, max_attempts: u8) -> bool {
    let mut attempts: u8 = 0;

    while attempts < max_attempts {
        attempts = attempts.saturating_add(1);

        if matches!(transport.ensure_connected().await, ConnectionOutcome::Connected) {
            return true;
        }

        Timer::after(CONNECT_RETRY_DELAY).await;
    }

    false
}

fn init_hardware() -> Peripherals {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    esp_alloc::heap_allocator!(size: 72 * 1024);
    esp_hal::init(config)
}
