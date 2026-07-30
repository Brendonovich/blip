use blip_media_time::FrameTimestamp;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioPacket {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    timestamp: Option<FrameTimestamp>,
}

impl AudioPacket {
    /// Creates a packet of interleaved 32-bit floating-point PCM samples.
    ///
    /// # Errors
    ///
    /// Returns an error when the sample rate or channel count is zero, or the
    /// sample count does not contain complete interleaved frames.
    pub fn new(
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u16,
        timestamp: Option<FrameTimestamp>,
    ) -> Result<Self, AudioPacketError> {
        if sample_rate == 0 {
            return Err(AudioPacketError::InvalidSampleRate);
        }
        if channels == 0 {
            return Err(AudioPacketError::InvalidChannelCount);
        }
        if samples.is_empty() {
            return Err(AudioPacketError::Empty);
        }
        if !samples.len().is_multiple_of(usize::from(channels)) {
            return Err(AudioPacketError::IncompleteFrame);
        }
        Ok(Self {
            samples,
            sample_rate,
            channels,
            timestamp,
        })
    }

    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.samples
            .len()
            .checked_div(usize::from(self.channels))
            .unwrap_or(0)
    }

    #[must_use]
    pub const fn timestamp(&self) -> Option<FrameTimestamp> {
        self.timestamp
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPacketError {
    InvalidSampleRate,
    InvalidChannelCount,
    Empty,
    IncompleteFrame,
}

impl std::fmt::Display for AudioPacketError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "audio sample rate must be greater than zero",
            Self::InvalidChannelCount => "audio channel count must be greater than zero",
            Self::Empty => "audio packet must contain at least one frame",
            Self::IncompleteFrame => "audio samples do not contain complete interleaved frames",
        })
    }
}

impl std::error::Error for AudioPacketError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_interleaved_frames() {
        assert!(AudioPacket::new(vec![0.0, 0.5, 1.0, -1.0], 48_000, 2, None).is_ok());
        assert_eq!(
            AudioPacket::new(vec![0.0], 48_000, 2, None),
            Err(AudioPacketError::IncompleteFrame)
        );
    }
}
