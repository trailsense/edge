extern crate alloc;
use alloc::vec::Vec;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use heapless::Vec as HeaplessVec;

const MAX_FINGERPRINTS: usize = 512; // TODO: Calculate how much we can actually fit

static FINGERPRINTS: Mutex<CriticalSectionRawMutex, RefCell<HeaplessVec<u64, MAX_FINGERPRINTS>>> =
    Mutex::new(RefCell::new(HeaplessVec::new()));

pub fn push(fingerprint: u64) -> bool {
    FINGERPRINTS.lock(|v| v.borrow_mut().push(fingerprint).is_ok())
}

pub fn drain() {
    FINGERPRINTS.lock(|v| v.borrow_mut().clear());
}

pub fn snapshot() -> Vec<u64> {
    FINGERPRINTS.lock(|v| v.borrow().iter().copied().collect())
}
