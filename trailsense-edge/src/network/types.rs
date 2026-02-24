#[derive(serde::Serialize, Debug)]
pub struct PackageDto<'a> {
    age_in_seconds: u64,
    count: u32,
    node_id: &'a str,
}

impl<'a> PackageDto<'a> {
    pub fn new(age_in_seconds: u64, count: u32, node_id: &'a str) -> Self {
        PackageDto {
            age_in_seconds,
            count,
            node_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendDataOutcome {
    Success,
    RetryableFailure,
    FatalFailure,
    BackoffRequired,
}

pub enum ConnectionOutcome {
    Connected,
    Disconnected,
    Failure,
}
