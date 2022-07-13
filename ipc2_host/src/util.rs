use std::time::SystemTime;

pub fn now() -> u128 {
    SystemTime::UNIX_EPOCH
        .elapsed()
        .expect("Failed to get current time")
        .as_millis()
}
