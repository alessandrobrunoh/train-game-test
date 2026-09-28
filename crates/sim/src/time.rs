//! Orologio di gioco. 1 tick = 1 minuto di gioco.

use std::fmt;
use std::ops::{Add, Sub};

use serde::{Deserialize, Serialize};

pub const MINUTES_PER_HOUR: u64 = 60;
pub const MINUTES_PER_DAY: u64 = 24 * MINUTES_PER_HOUR;

/// Absolute game time, in minutes since day 1 at 00:00.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct GameTime(pub u64);

impl GameTime {
    /// Builds a time from a 1-based day number, hour and minute.
    pub fn from_dhm(day: u64, hour: u64, minute: u64) -> Self {
        Self(day.saturating_sub(1) * MINUTES_PER_DAY + hour * MINUTES_PER_HOUR + minute)
    }

    pub fn minutes(self) -> u64 {
        self.0
    }

    /// 1-based day number ("Giorno 1" is the first day).
    pub fn day(self) -> u64 {
        self.0 / MINUTES_PER_DAY + 1
    }

    pub fn hour(self) -> u32 {
        ((self.0 % MINUTES_PER_DAY) / MINUTES_PER_HOUR) as u32
    }

    pub fn minute(self) -> u32 {
        (self.0 % MINUTES_PER_HOUR) as u32
    }

    /// Minutes elapsed since midnight of the current day (0..1440).
    pub fn minute_of_day(self) -> u32 {
        (self.0 % MINUTES_PER_DAY) as u32
    }

    /// Hour of day as a fraction, e.g. 13:30 -> 13.5.
    pub fn hour_f(self) -> f32 {
        self.minute_of_day() as f32 / MINUTES_PER_HOUR as f32
    }

    /// The first moment strictly after `self` whose time of day is `hour:minute`.
    pub fn next_at(self, hour: u32, minute: u32) -> GameTime {
        let target = (hour * 60 + minute) as u64;
        let today = self.0 - self.0 % MINUTES_PER_DAY;
        let candidate = today + target;
        if candidate > self.0 {
            GameTime(candidate)
        } else {
            GameTime(candidate + MINUTES_PER_DAY)
        }
    }

    /// Minutes from `earlier` to `self` (0 if `earlier` is later).
    pub fn since(self, earlier: GameTime) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl Add<u64> for GameTime {
    type Output = GameTime;
    fn add(self, minutes: u64) -> GameTime {
        GameTime(self.0 + minutes)
    }
}

impl Sub<GameTime> for GameTime {
    type Output = u64;
    fn sub(self, rhs: GameTime) -> u64 {
        self.since(rhs)
    }
}

impl fmt::Display for GameTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Giorno {} {:02}:{:02}",
            self.day(),
            self.hour(),
            self.minute()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parts() {
        let t = GameTime::from_dhm(3, 8, 15);
        assert_eq!(t.to_string(), "Giorno 3 08:15");
        assert_eq!((t.day(), t.hour(), t.minute()), (3, 8, 15));
        assert!((t.hour_f() - 8.25).abs() < 1e-6);
    }

    #[test]
    fn next_at_wraps_to_tomorrow() {
        let t = GameTime::from_dhm(1, 22, 0);
        assert_eq!(t.next_at(6, 0), GameTime::from_dhm(2, 6, 0));
        assert_eq!(t.next_at(23, 0), GameTime::from_dhm(1, 23, 0));
        assert_eq!(t.next_at(22, 0), GameTime::from_dhm(2, 22, 0));
    }
}
