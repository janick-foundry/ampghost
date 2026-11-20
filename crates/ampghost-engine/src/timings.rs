use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub identity_timeout: Duration,
    pub read_timeout: Duration,
    pub pre_request_drain: Duration,
    pub packet_gap: Duration,
    pub settle: Duration,
    pub channel_settle: Duration,
    pub channel_attempts: u8,
    pub program_change_holdoff: Duration,
    pub dump_quiet: Duration,
    pub dump_limit: Duration,
    pub read_attempts: u8,
    pub port_check: Duration,
    pub connect_attempts: u8,
    pub backup_gap: Duration,
    pub backup_timeout: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            identity_timeout: Duration::from_millis(2500),
            read_timeout: Duration::from_millis(1500),
            pre_request_drain: Duration::from_millis(15),
            packet_gap: Duration::from_millis(20),
            settle: Duration::from_millis(300),
            channel_settle: Duration::from_millis(700),
            channel_attempts: 3,
            program_change_holdoff: Duration::from_millis(3000),
            dump_quiet: Duration::from_millis(500),
            dump_limit: Duration::from_millis(3000),
            read_attempts: 2,
            port_check: Duration::from_secs(3),
            connect_attempts: 3,
            backup_gap: Duration::from_millis(25),
            backup_timeout: Duration::from_secs(5),
        }
    }
}
