extern crate alloc;
use alloc::vec::Vec;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

static FINGERPRINTS: Mutex<CriticalSectionRawMutex, RefCell<Vec<u16>>> =
    Mutex::new(RefCell::new(Vec::new()));

pub fn push(fingerprint: u16) {
    FINGERPRINTS.lock(|v| v.borrow_mut().push(fingerprint));
}

pub fn drain() {
    FINGERPRINTS.lock(|v| v.borrow_mut().clear());
}

pub fn snapshot() -> Vec<u16> {
    FINGERPRINTS.lock(|v| v.borrow().clone())
}
