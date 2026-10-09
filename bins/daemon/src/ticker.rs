//! The tick timer: a periodic `timerfd`, so each tick costs one wakeup.
//!
//! A 5 s `tokio::time::sleep` wakes the process three times: tokio's timer wheel keeps long
//! deadlines in coarse levels and moves them down a level at each expiry (4.1 s, then 64 ms
//! slots). The kernel timer fires once, and keeps ticks on a fixed grid instead of drifting by
//! each tick's own duration.

use std::io;
use std::os::fd::OwnedFd;
use std::time::Duration;

use rustix::io::Errno;
use rustix::time::{
    Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, timerfd_create,
    timerfd_settime,
};
use tokio::io::unix::AsyncFd;

#[derive(Debug)]
pub struct Ticker {
    timer: AsyncFd<OwnedFd>,
}

impl Ticker {
    /// Fires every `period` on `CLOCK_MONOTONIC`, first one `period` from now. Needs a tokio
    /// runtime with IO enabled.
    ///
    /// # Errors
    /// If the timer can't be created or registered.
    pub fn new(period: Duration) -> io::Result<Self> {
        let timer = timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::NONBLOCK | TimerfdFlags::CLOEXEC,
        )?;
        arm(&timer, period, period)?;
        Ok(Self {
            timer: AsyncFd::new(timer)?,
        })
    }

    /// Fires next after `first` (at once if zero), then every `period`.
    ///
    /// # Errors
    /// If the timer can't be set.
    pub fn set_period(&self, period: Duration, first: Duration) -> io::Result<()> {
        // A zero first expiration would disarm the timer.
        arm(
            self.timer.get_ref(),
            period,
            first.max(Duration::from_nanos(1)),
        )
    }

    /// Waits for the next tick. Returns how many periods elapsed since the last call; more
    /// than one only when a tick ran late. Cancel-safe.
    ///
    /// # Errors
    /// If reading the timer fails.
    pub async fn tick(&self) -> io::Result<u64> {
        loop {
            let mut ready = self.timer.readable().await?;
            let mut expirations = [0u8; 8];
            match rustix::io::read(self.timer.get_ref(), &mut expirations) {
                Ok(_) => return Ok(u64::from_ne_bytes(expirations)),
                Err(Errno::AGAIN) => ready.clear_ready(),
                Err(err) => return Err(err.into()),
            }
        }
    }
}

fn arm(timer: &OwnedFd, period: Duration, first: Duration) -> io::Result<()> {
    let spec = |d: Duration| Timespec {
        tv_sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(d.subsec_nanos()),
    };
    timerfd_settime(
        timer,
        TimerfdTimerFlags::empty(),
        &Itimerspec {
            it_interval: spec(period),
            it_value: spec(first),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[tokio::test]
    async fn fires_every_period() {
        let ticker = Ticker::new(Duration::from_millis(20)).unwrap();
        let start = Instant::now();
        assert_eq!(ticker.tick().await.unwrap(), 1);
        assert_eq!(ticker.tick().await.unwrap(), 1);
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn changes_period() {
        let ticker = Ticker::new(Duration::from_secs(60)).unwrap();
        let start = Instant::now();
        ticker
            .set_period(Duration::from_millis(10), Duration::ZERO)
            .unwrap();
        assert_eq!(ticker.tick().await.unwrap(), 1);
        ticker.tick().await.unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn counts_periods_missed_by_a_late_reader() {
        let ticker = Ticker::new(Duration::from_millis(10)).unwrap();
        std::thread::sleep(Duration::from_millis(35));
        assert!(ticker.tick().await.unwrap() >= 3);
    }
}
