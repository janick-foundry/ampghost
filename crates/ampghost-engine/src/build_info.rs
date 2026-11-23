#[cfg(feature = "native")]
use std::time::{Duration, UNIX_EPOCH};

pub const BUILD_COMMIT: &str = env!("AMPGHOST_BUILD_COMMIT");
pub const VERSION_LABEL: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AMPGHOST_BUILD_COMMIT"),
    ")"
);

#[cfg(feature = "native")]
pub fn build_time() -> String {
    match env!("AMPGHOST_BUILD_EPOCH").parse::<u64>() {
        Ok(seconds) if seconds > 0 => {
            crate::native::rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds))
        }
        _ => "unknown".to_owned(),
    }
}
