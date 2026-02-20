extern crate alloc;
use alloc::vec::Vec;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::Instant;
use heapless::Vec as HeaplessVec;

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

const MAX_PACKAGES: usize = 64;

static PACKAGES: Mutex<CriticalSectionRawMutex, RefCell<HeaplessVec<PackageEntity, MAX_PACKAGES>>> =
    Mutex::new(RefCell::new(HeaplessVec::new()));

pub async fn push(count: u32) -> bool {
    PACKAGES.lock(|v| {
        let mut packages = v.borrow_mut();
        let entity = PackageEntity::new(count);

        if packages.push(entity).is_ok() {
            return true;
        }

        // Evict oldest buffered package to keep a bounded queue.
        // TODO: think about better solution, chunking, deleting values in between? Drop a few 0 count values?
        // Thought appeared on 20.02.2026 --> If is 0, do delete it, only if enough other values are around it maybe?ß
        if !packages.is_empty() {
            packages.remove(0);
        }

        packages.push(PackageEntity::new(count)).is_ok()
    })
}

pub async fn snapshot_with_age() -> Vec<PackageEntity> {
    PACKAGES.lock(|v| {
        let mut packages = v.borrow_mut();
        for p in packages.iter_mut() {
            p.update_age();
        }
        packages.iter().cloned().collect()
    })
}

pub async fn drain() {
    PACKAGES.lock(|v| v.borrow_mut().clear());
}
