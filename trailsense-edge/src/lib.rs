#![no_std]

#[cfg(all(feature = "uplink-wifi", feature = "uplink-gsm"))]
compile_error!("Features `uplink-wifi` and `uplink-gsm` are mutually exclusive. Enable only one.");

#[cfg(not(any(feature = "uplink-wifi", feature = "uplink-gsm")))]
compile_error!("No uplink transport selected. Enable `uplink-gsm` (default) or `uplink-wifi`.");

// TODO: This has been deactivated/unwired since not needed for now. Could be needed for sleep etc.
// #[cfg(feature = "uplink-gsm")]
// pub mod hardware;
pub mod lora;
pub mod network;
pub mod orchestration;
pub mod packages;
pub mod probes;
pub mod wifi;
