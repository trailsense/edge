extern crate alloc;
use alloc::vec::Vec;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Instant;

#[derive(Debug, Clone)]
pub struct PackageEntity {
    pub count: u32,
    pub age_in_seconds: u64,
    pub last_seen: Instant,
}

impl PackageEntity {
    pub fn new(count: u32) -> Self {
        Self {
            count,
            age_in_seconds: 0,
            last_seen: Instant::now(),
        }
    }

    pub fn update_age(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_seen);
        self.age_in_seconds = self.age_in_seconds.saturating_add(delta.as_secs());
        self.last_seen = now;
    }
}

static PACKAGES: Mutex<CriticalSectionRawMutex, Vec<PackageEntity>> = Mutex::new(Vec::new());

pub async fn push(count: u32) {
    let mut guard = PACKAGES.lock().await;
    guard.push(PackageEntity::new(count));
}

pub async fn snapshot_with_age() -> Vec<PackageEntity> {
    let mut guard = PACKAGES.lock().await;
    for p in guard.iter_mut() {
        p.update_age();
    }
    guard.clone()
}

pub async fn drain() {
    let mut guard = PACKAGES.lock().await;
    guard.clear();
}
