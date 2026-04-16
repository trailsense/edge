use core::fmt::{self, Write};

pub const BASE_URL: &str = env!("TRAILSENSE_API_URL");
pub const DEVICE_ID: &str = env!("TRAILSENSE_EDGE_ID");

pub fn ingest_url() -> Result<heapless::String<128>, fmt::Error> {
    let mut url = heapless::String::<128>::new();
    write!(&mut url, "{}/ingest", BASE_URL)?;
    Ok(url)
}
