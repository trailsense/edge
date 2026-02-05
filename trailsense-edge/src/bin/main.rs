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
use log::info;
use static_cell::StaticCell;
use trailsense_edge::{
    probe_parser::read_packet,
    wifi::{self, manager::WifiCmd},
};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static RADIO_CELL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static WIFI_COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, WifiCmd, 4> = Channel::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.1.0
    let peripherals = init_hardware();

    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let radio = RADIO_CELL
        .uninit()
        .write(esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller"));

    let (wifi_controller, interfaces) =
        esp_radio::wifi::new(radio, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    info!("Trailsense node is up");
    info!("Starting Wifi Setup");

    let mut rng = Rng::new();
    let (ctx, runner) = wifi::init_stack(&mut rng, interfaces.sta);

    spawner
        .spawn(wifi::tasks::connect(wifi_controller))
        .unwrap();
    spawner.spawn(wifi::tasks::net_task(runner)).unwrap();

    info!("Connection is up");

    spawner
        .spawn(wifi::uploader::uploader_task(
            ctx,
            WIFI_COMMAND_CHANNEL.sender(),
        ))
        .unwrap();

    spawner
        .spawn(wifi::manager::wifi_manager_task(
            interfaces.sniffer,
            read_packet,
            WIFI_COMMAND_CHANNEL.receiver(),
        ))
        .unwrap();

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
