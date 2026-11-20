use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;

    fn sleep(&self, duration: Duration);

    fn timestamp(&self) -> String;
}
