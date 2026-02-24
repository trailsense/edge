#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::Peripherals;
use esp_hal::rng::Rng;

use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;
use log::{error, info};
use static_cell::StaticCell;
use trailsense_edge::{
    probes::probe_parser::read_packet,
    wifi::{self, manager::WifiCmd, tasks::WifiControlCmd},
};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO_CELL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static WIFI_COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, WifiCmd, 4> = Channel::new();
static WIFI_CONTROL_CHANNEL: Channel<CriticalSectionRawMutex, WifiControlCmd, 4> = Channel::new();
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

    let radio = RADIO_CELL.uninit().write(radio_init);

    let (wifi_controller, interfaces) =
        match esp_radio::wifi::new(radio, peripherals.WIFI, Default::default()) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to initialize Wi-Fi controller (fatal): {:?}", e);
                fatal_idle().await;
            }
        };

    info!("Trailsense node is up");
    info!("Starting Wifi Setup");

    let mut rng = Rng::new();
    let (ctx, runner) = wifi::init_stack(&mut rng, interfaces.sta);

    if let Err(e) = spawner.spawn(wifi::tasks::connect(
        wifi_controller,
        WIFI_CONTROL_CHANNEL.receiver(),
    )) {
        error!("Failed to spawn connection task: {}", e);
    }

    if let Err(e) = spawner.spawn(wifi::tasks::net_task(runner)) {
        error!("Failed to spawn net task: {}", e);
    }

    info!("Connection is up");

    if let Err(e) = spawner.spawn(wifi::uploader::uploader_task(
        ctx,
        WIFI_COMMAND_CHANNEL.sender(),
        WIFI_CONTROL_CHANNEL.sender(),
    )) {
        error!("Failed to spawn uploader task: {}", e);
    }

    if let Err(e) = spawner.spawn(wifi::manager::wifi_manager_task(
        interfaces.sniffer,
        read_packet,
        WIFI_COMMAND_CHANNEL.receiver(),
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
