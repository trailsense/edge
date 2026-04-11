#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::PubSubChannel,
};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::Peripherals;

#[cfg(feature = "uplink-wifi")]
use esp_hal::rng::Rng;

use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;

#[cfg(feature = "uplink-gsm")]
use esp_hal::uart::{Config, Uart};
use log::{error, info};
#[cfg(feature = "uplink-wifi")]
use static_cell::StaticCell;
use trailsense_edge::{
    network::{self},
    orchestration::{
        orchestrator::orchestrate_node,
        types::{SystemCmd, SystemEvents},
    },
    probes::probe_parser::read_packet,
    wifi::{self},
};

#[cfg(feature = "uplink-gsm")]
use trailsense_edge::network::factory::build_active_transport_gsm;
#[cfg(feature = "uplink-gsm")]
use trailsense_edge::network::gsm::UART_BAUDRATE;
#[cfg(feature = "uplink-wifi")]
use trailsense_edge::network::factory::build_active_transport_wifi;
#[cfg(feature = "uplink-wifi")]
use trailsense_edge::wifi::tasks::WifiControlCmd;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[cfg(feature = "uplink-wifi")]
static RADIO_CELL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
#[cfg(feature = "uplink-wifi")]
static WIFI_CONTROL_CHANNEL: Channel<CriticalSectionRawMutex, WifiControlCmd, 4> = Channel::new();

static SNIFFING_COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, SystemCmd, 4> = Channel::new();
static NETWORK_COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, SystemCmd, 4> = Channel::new();
static ORCHESTRATOR_EVENT_CHANNEL: PubSubChannel<CriticalSectionRawMutex, SystemEvents, 4, 1, 3> =
    PubSubChannel::new();

#[cfg(feature = "uplink-wifi")]
const INIT_RETRY_DELAY: Duration = Duration::from_secs(5);
const FATAL_SLEEP: Duration = Duration::from_secs(1);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]

async fn fatal_idle() -> ! {
    loop {
        Timer::after(FATAL_SLEEP).await;
    }
}
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.1.0
    let peripherals = init_hardware();

    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("Trailsense node is up");

    #[cfg(feature = "uplink-gsm")]
    let uart = match Uart::new(
        peripherals.UART2,
        Config::default().with_baudrate(UART_BAUDRATE),
    ) {
        Ok(u) => u,
        Err(e) => {
            error!("Failed to initialize GSM UART (fatal): {:?}", e);
            fatal_idle().await;
        }
    }
    .with_tx(peripherals.GPIO17)
    .with_rx(peripherals.GPIO16)
    .into_async();

    #[cfg(feature = "uplink-gsm")]
    let transport = match build_active_transport_gsm(uart, spawner) {
        Ok(t) => t,
        // TODO: (hw-reset): wire ESP32 GPIO to modem RESET/PWRKEY and attempt hardware reset + reinit before fatal_idle().
        Err(e) => {
            error!("Issue initializing GSM module (fatal): {:?}", e);
            fatal_idle().await;
        }
    };

    #[cfg(feature = "uplink-wifi")]
    info!("Starting Wifi Setup");

    #[cfg(feature = "uplink-wifi")]
    let radio_init = loop {
        match esp_radio::init() {
            Ok(r) => break r,
            Err(e) => {
                error!(
                    "Failed to initialize Wi-Fi/BLE controller; retrying in {:?}: {:?}",
                    INIT_RETRY_DELAY, e
                );
                Timer::after(INIT_RETRY_DELAY).await;
            }
        }
    };

    #[cfg(feature = "uplink-wifi")]
    let radio = RADIO_CELL.uninit().write(radio_init);

    #[cfg(feature = "uplink-wifi")]
    let (wifi_controller, interfaces) =
        match esp_radio::wifi::new(radio, peripherals.WIFI, Default::default()) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to initialize Wi-Fi controller (fatal): {:?}", e);
                fatal_idle().await;
            }
        };

    #[cfg(feature = "uplink-wifi")]
    let mut rng = Rng::new();
    #[cfg(feature = "uplink-wifi")]
    let (ctx, runner) = wifi::init_stack(&mut rng, interfaces.sta);

    #[cfg(feature = "uplink-wifi")]
    let orchestrator_transport_publisher = match ORCHESTRATOR_EVENT_CHANNEL.publisher() {
        Ok(p) => p,
        Err(e) => {
            error!(
                "Failed to get publisher for ORCHESTRATOR_EVENT_CHANNEL (fatal): {:?}",
                e
            );

            fatal_idle().await;
        }
    };

    #[cfg(feature = "uplink-wifi")]
    if let Err(e) = spawner.spawn(wifi::tasks::connect(
        wifi_controller,
        WIFI_CONTROL_CHANNEL.receiver(),
        orchestrator_transport_publisher,
    )) {
        error!("Failed to spawn connection task: {}", e);
    }

    #[cfg(feature = "uplink-wifi")]
    if let Err(e) = spawner.spawn(wifi::tasks::net_task(runner)) {
        error!("Failed to spawn net task: {}", e);
    }

    #[cfg(feature = "uplink-wifi")]
    info!("Connection is up");

    #[cfg(feature = "uplink-wifi")]
    let transport = build_active_transport_wifi(ctx, WIFI_CONTROL_CHANNEL.sender());

    let orchestrator_network_publisher = match ORCHESTRATOR_EVENT_CHANNEL.publisher() {
        Ok(p) => p,
        Err(e) => {
            error!(
                "Failed to acquire publisher for ORCHESTRATOR_EVENT_CHANNEL (fatal): {:?}",
                e
            );

            fatal_idle().await;
        }
    };

    if let Err(e) = spawner.spawn(network::uploader::uploader_task(
        transport,
        NETWORK_COMMAND_CHANNEL.receiver(),
        orchestrator_network_publisher,
    )) {
        error!("Failed to spawn uploader task: {}", e);
    }

    let orchestrator_sniffer_publisher = match ORCHESTRATOR_EVENT_CHANNEL.publisher() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to initialize Wi-Fi controller (fatal): {:?}", e);
            fatal_idle().await;
        }
    };

    let orchestrator_event_subscriber = match ORCHESTRATOR_EVENT_CHANNEL.subscriber() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to initialize Wi-Fi controller (fatal): {:?}", e);
            fatal_idle().await;
        }
    };

    if let Err(e) = spawner.spawn(orchestrate_node(
        orchestrator_event_subscriber,
        NETWORK_COMMAND_CHANNEL.sender(),
        SNIFFING_COMMAND_CHANNEL.sender(),
        trailsense_edge::orchestration::types::SystemState::Idle,
    )) {
        error!("Failed to spawn wifi manager task: {}", e);
    }

    #[cfg(feature = "uplink-wifi")]
    if let Err(e) = spawner.spawn(wifi::manager::wifi_manager_task(
        interfaces.sniffer,
        read_packet,
        SNIFFING_COMMAND_CHANNEL.receiver(),
        orchestrator_sniffer_publisher,
    )) {
        error!("Failed to spawn wifi manager task: {}", e);
    }

    #[cfg(feature = "uplink-gsm")]
    if let Err(e) = spawner.spawn(wifi::manager::gsm_wifi_manager_task(
        read_packet,
        SNIFFING_COMMAND_CHANNEL.receiver(),
        orchestrator_sniffer_publisher,
    )) {
        error!("Failed to spawn wifi manager task: {}", e);
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

fn init_hardware() -> Peripherals {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 72 * 1024);
    peripherals
}
