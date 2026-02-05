extern crate alloc;
use alloc::vec::Vec;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use heapless::Vec as HeaplessVec;

const MAX_FINGERPRINTS: usize = 2048;

static FINGERPRINTS: Mutex<CriticalSectionRawMutex, RefCell<HeaplessVec<u16, MAX_FINGERPRINTS>>> =
    Mutex::new(RefCell::new(HeaplessVec::new()));

pub fn push(fingerprint: u16) -> bool {
    FINGERPRINTS.lock(|v| v.borrow_mut().push(fingerprint).is_ok())
}

pub fn drain() {
    FINGERPRINTS.lock(|v| v.borrow_mut().clear());
}

pub fn snapshot() -> Vec<u16> {
    FINGERPRINTS.lock(|v| v.borrow().iter().copied().collect())
}
