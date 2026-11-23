use std::time::{Duration, UNIX_EPOCH};

use super::rfc3339_seconds;

#[test]
fn formats_known_instants() {
    for (seconds, expected) in [
        (0, "1970-01-01T00:00:00Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_709_164_800, "2024-02-29T00:00:00Z"),
        (4_102_444_799, "2099-12-31T23:59:59Z"),
        (253_402_300_799, "9999-12-31T23:59:59Z"),
    ] {
        assert_eq!(
            rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds)),
            expected
        );
    }
}

#[test]
fn clamps_times_before_the_epoch() {
    let before = UNIX_EPOCH.checked_sub(Duration::from_secs(1));
    assert_eq!(
        before.map(rfc3339_seconds).as_deref(),
        Some("1970-01-01T00:00:00Z")
    );
}
