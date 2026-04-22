#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::PubSubChannel,
};
use embassy_time::Delay;
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::peripherals::Peripherals;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262};

#[cfg(feature = "uplink-wifi")]
use esp_hal::rng::Rng;

use static_cell::StaticCell;
use trailsense_edge::lora;
use trailsense_edge::probes::models::{DEDUP_MODEL_VERSION, MODEL_SIZE, TAU};

use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;

#[cfg(feature = "uplink-gsm")]
use esp_hal::uart::{Config, Uart};
use log::{error, info};
#[cfg(feature = "uplink-wifi")]
use trailsense_edge::{
    network::{self},
    orchestration::{
        orchestrator::orchestrate_node,
        types::{SystemCmd, SystemEvents},
    },
    probes::{counter::validate_runtime_model_config, probe_parser::read_packet},
    wifi::{self},
};

#[cfg(feature = "uplink-gsm")]
use trailsense_edge::network::factory::build_active_transport_gsm;
#[cfg(feature = "uplink-wifi")]
use trailsense_edge::network::factory::build_active_transport_wifi;
#[cfg(feature = "uplink-gsm")]
use trailsense_edge::network::gsm::UART_BAUDRATE;
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

static SPI_BUS: StaticCell<
    Mutex<CriticalSectionRawMutex, esp_hal::spi::master::Spi<'static, Async>>,
> = StaticCell::new();

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
        Err(e) => {
            error!("Issue initializing GSM module: {:?}", e);
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
        peripherals.WIFI,
        orchestrator_sniffer_publisher,
    )) {
        error!("Failed to spawn wifi manager task: {}", e);
    }

    match validate_runtime_model_config() {
        true => info!(
            "DEDUP config is working: version = {}, bits = {}, tau={}",
            DEDUP_MODEL_VERSION, MODEL_SIZE, TAU
        ),
        false => {
            error! {"DEDUP config is not correct."}
        }
    }

    let spi = Spi::new(peripherals.SPI3, SpiConfig::default())
        .expect("failed to create SPI")
        .with_sck(peripherals.GPIO18)
        .with_mosi(peripherals.GPIO23)
        .with_miso(peripherals.GPIO19)
        .into_async();

    // LoRa control pins.
    let nss = Output::new(peripherals.GPIO5, Level::High, OutputConfig::default());
    let reset = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let dio1 = Input::new(peripherals.GPIO33, InputConfig::default());
    let busy = Input::new(peripherals.GPIO32, InputConfig::default());
    let rx_en = Output::new(peripherals.GPIO25, Level::Low, OutputConfig::default());
    let tx_en = Output::new(peripherals.GPIO26, Level::Low, OutputConfig::default());

    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, Some(rx_en), Some(tx_en))
        .expect("failed to create SX126x interface variant");

    let spi_bus = SPI_BUS.init(Mutex::new(spi));
    let spi_device = embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice::new(spi_bus, nss);

    let radio = Sx126x::new(
        spi_device,
        iv,
        Sx126xConfig {
            chip: Sx1262,
            tcxo_ctrl: None,
            use_dcdc: false,
            rx_boost: false,
        },
    );

    #[cfg(feature = "lora-gateway")]
    let lora: LoRa<_, _> = LoRa::new(radio, false, Delay)
        .await
        .expect("failed to init LoRa");

    if let Err(e) = spawner.spawn(lora::tasks::recieve_lora_packets(lora)) {
        error!("Failed to spawn lora receive task: {}", e);
    };

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
