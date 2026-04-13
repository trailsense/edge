use core::fmt::{self, Write};

pub const BASE_URL: &str = match option_env!("TRAILSENSE_API_URL") {
    Some(v) => v,
    None => "https://api.trailsense.daugt.com",
};

pub const DEVICE_ID: &str = match option_env!("TRAILSENSE_EDGE_ID") {
    Some(v) => v,
    None => "71ec4873-944e-49c1-b7c4-4b856797715f",
};

pub fn ingest_url() -> Result<heapless::String<128>, fmt::Error> {
    let mut url = heapless::String::<128>::new();
    write!(&mut url, "{}/ingest", BASE_URL)?;
    Ok(url)
}
