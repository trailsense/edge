#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::main;
use esp_hal::peripherals::Peripherals;

use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
use esp_radio::wifi::PromiscuousPkt;
use ieee80211::GenericFrame;
use ieee80211::common::{FrameType, ManagementFrameSubtype};
use log::info;
use trailsense_edge::probe_parser::fingerprint_probe;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    // generator version: 1.1.0

    let peripherals = init_hardware();

    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    let radio_init = esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller");

    let _transport = BleConnector::new(&radio_init, peripherals.BT, Default::default()).unwrap(); // Due to some misterious bug from the esp_radio, it is necessary to setup BLE, even when not in use. This issue describes how to fix it https://github.com/espressif/esp-idf/issues/13113
    let (mut _wifi_controller, interfaces) =
        esp_radio::wifi::new(&radio_init, peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    let mut device = interfaces.sniffer;

    device
        .set_promiscuous_mode(true)
        .expect("Failed to set wifi into sniffer mode");

    device.set_receive_cb(read_packet);

    loop {}
}

fn init_hardware() -> Peripherals {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 72 * 1024);
    peripherals
}

fn read_packet(packet: PromiscuousPkt) {
    let Ok(frame) = GenericFrame::new(&packet.data, false) else {
        return;
    };

    if let Some(source) = frame.address_2() {
        if !((source[0] == 84 && source[1] == 138 && source[2] == 186) // FOR TESTING PURPOSES: Filter out both CISCO and ESPRESSIF MAC-Addresses, to visualize "normal" devices
            || (source[0] == 52 && source[1] == 152 && source[2] == 122) || (source[0] == 112 && source[1] == 211 && source[2] == 121) || (source[0] == 16 && source[1] == 60 && source[2] == 89))
        {
            let fc = frame.frame_control_field();
            if let FrameType::Management(subtype) = fc.frame_type() {
                if subtype == ManagementFrameSubtype::ProbeRequest {
                    let body_offset = 24;
                    let body = &packet.data[body_offset..];

                    let fingerprint = fingerprint_probe(body);

                    info!(
                        "Source MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        source[0], source[1], source[2], source[3], source[4], source[5]
                    );
                    info!("Probe body[0..16]: {:02x?}", &body[0..body.len().min(16)]);

                    info!("Fingerprint: {:08b}", fingerprint);
                }
            }
        }
    }
}
