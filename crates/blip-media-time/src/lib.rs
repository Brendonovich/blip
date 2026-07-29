use std::time::Duration;

/// A media timestamp mapped into the platform's shared monotonic clock domain.
///
/// Capture backends must convert source-specific clocks before constructing this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameTimestamp(Duration);

impl FrameTimestamp {
    #[must_use]
    pub const fn from_duration_since_epoch(timestamp: Duration) -> Self {
        Self(timestamp)
    }

    #[must_use]
    pub fn from_ratio(value: i64, timescale: i32) -> Option<Self> {
        if value < 0 || timescale <= 0 {
            return None;
        }
        let nanos = i128::from(value)
            .checked_mul(1_000_000_000)?
            .checked_div(i128::from(timescale))?;
        Some(Self(Duration::from_nanos(u64::try_from(nanos).ok()?)))
    }

    #[must_use]
    pub const fn duration_since_epoch(self) -> Duration {
        self.0
    }

    #[must_use]
    pub fn signed_seconds_since(self, earlier: Self) -> f64 {
        if self >= earlier {
            self.0.saturating_sub(earlier.0).as_secs_f64()
        } else {
            -earlier.0.saturating_sub(self.0).as_secs_f64()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::FrameTimestamp;

    #[test]
    fn converts_rational_platform_time_without_floating_point() {
        assert_eq!(
            FrameTimestamp::from_ratio(3, 2),
            Some(FrameTimestamp::from_duration_since_epoch(
                Duration::from_millis(1500)
            ))
        );
    }

    #[test]
    fn calculates_signed_offsets() {
        let early = FrameTimestamp::from_duration_since_epoch(Duration::from_millis(250));
        let late = FrameTimestamp::from_duration_since_epoch(Duration::from_millis(500));
        assert!((late.signed_seconds_since(early) - 0.25).abs() < f64::EPSILON);
        assert!((early.signed_seconds_since(late) + 0.25).abs() < f64::EPSILON);
    }
}
