use blip_media_time::FrameTimestamp;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use std::sync::Arc;

pub const MIX_SAMPLE_RATE: u32 = 48_000;
pub const MIX_CHANNELS: u16 = 2;

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

#[derive(Clone, Debug)]
pub struct AudioSource {
    samples: Arc<[f32]>,
    start_offset_secs: f64,
    gain: f32,
}

impl AudioSource {
    /// Creates a normalized 48 kHz stereo source positioned on the recording timeline.
    ///
    /// # Errors
    ///
    /// Returns an error if `samples` does not contain complete stereo frames.
    pub fn new(
        samples: Vec<f32>,
        start_offset_secs: f64,
        gain: f32,
    ) -> Result<Self, AudioPacketError> {
        if !samples.len().is_multiple_of(usize::from(MIX_CHANNELS)) {
            return Err(AudioPacketError::IncompleteFrame);
        }
        Ok(Self {
            samples: samples.into(),
            start_offset_secs,
            gain,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioTimelineSegment {
    pub source_start_secs: f64,
    pub source_end_secs: f64,
}

impl AudioTimelineSegment {
    #[must_use]
    pub fn duration_secs(self) -> f64 {
        (self.source_end_secs - self.source_start_secs).max(0.0)
    }
}

#[derive(Clone, Debug)]
pub struct AudioMixer {
    sources: Vec<AudioSource>,
    segments: Vec<AudioTimelineSegment>,
}

impl AudioMixer {
    #[must_use]
    pub fn new(sources: Vec<AudioSource>, segments: Vec<AudioTimelineSegment>) -> Self {
        Self { sources, segments }
    }

    #[must_use]
    pub fn has_sources(&self) -> bool {
        !self.sources.is_empty()
    }

    /// Renders a packet beginning at an editor-timeline time.
    ///
    /// # Errors
    ///
    /// Returns an error when `frame_count` is zero.
    pub fn render(
        &self,
        timeline_start_secs: f64,
        frame_count: usize,
    ) -> Result<AudioPacket, AudioPacketError> {
        let channel_count = usize::from(MIX_CHANNELS);
        let mut output = vec![0.0; frame_count.saturating_mul(channel_count)];
        for (output_frame, output_samples) in output.chunks_exact_mut(channel_count).enumerate() {
            let timeline_time = timeline_start_secs
                + f64::from(u32::try_from(output_frame).unwrap_or(u32::MAX))
                    / f64::from(MIX_SAMPLE_RATE);
            output_samples.copy_from_slice(&self.mix_frame_at(timeline_time));
        }
        AudioPacket::new(output, MIX_SAMPLE_RATE, MIX_CHANNELS, None)
    }

    fn mix_frame_at(&self, timeline_time_secs: f64) -> [f32; 2] {
        let Some(source_time) = self.source_time_at(timeline_time_secs) else {
            return [0.0; 2];
        };
        let mut output = [0.0_f32; 2];
        for source in &self.sources {
            let input_time = source_time - source.start_offset_secs;
            let Some(input_frame) = frame_index_at(input_time) else {
                continue;
            };
            let Some(input_start) = input_frame.checked_mul(usize::from(MIX_CHANNELS)) else {
                continue;
            };
            for (channel, mixed) in output.iter_mut().enumerate() {
                let Some(input_index) = input_start.checked_add(channel) else {
                    continue;
                };
                if let Some(input) = source.samples.get(input_index) {
                    *mixed += input * source.gain;
                }
            }
        }
        output.map(|sample| sample.clamp(-1.0, 1.0))
    }

    fn source_time_at(&self, timeline_time_secs: f64) -> Option<f64> {
        if timeline_time_secs < 0.0 {
            return None;
        }
        let mut timeline_start = 0.0;
        for segment in &self.segments {
            let duration = segment.duration_secs();
            let timeline_end = timeline_start + duration;
            if timeline_time_secs < timeline_end {
                return Some(segment.source_start_secs + timeline_time_secs - timeline_start);
            }
            timeline_start = timeline_end;
        }
        None
    }

    #[must_use]
    pub fn duration_secs(&self) -> f64 {
        self.segments
            .iter()
            .map(|segment| segment.duration_secs())
            .sum()
    }
}

fn frame_index_at(time_secs: f64) -> Option<usize> {
    if !time_secs.is_finite() || time_secs < 0.0 {
        return None;
    }
    let nanos = std::time::Duration::try_from_secs_f64(time_secs)
        .ok()?
        .as_nanos();
    let frames = nanos
        .checked_mul(u128::from(MIX_SAMPLE_RATE))?
        .checked_add(500_000_000)?
        .checked_div(1_000_000_000)?;
    usize::try_from(frames).ok()
}

pub struct AudioPlayback {
    _stream: cpal::Stream,
}

impl AudioPlayback {
    /// Starts playback of a pre-rendered mixer packet on the default output device.
    ///
    /// # Errors
    ///
    /// Returns an error if no output device is available or its stream cannot be started.
    pub fn start(mixer: AudioMixer, timeline_start_secs: f64) -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "no default audio output device is available".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|error| error.to_string())?;
        let config: cpal::StreamConfig = supported.clone().into();
        let start_frame = frame_index_at(timeline_start_secs)
            .and_then(|frame| u64::try_from(frame).ok())
            .ok_or_else(|| "audio playback start time is out of range".to_owned())?;
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::F64 => {
                build_output_stream::<f64>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::I8 => {
                build_output_stream::<i8>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::I16 => {
                build_output_stream::<i16>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::I32 => {
                build_output_stream::<i32>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::I64 => {
                build_output_stream::<i64>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::U8 => {
                build_output_stream::<u8>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::U16 => {
                build_output_stream::<u16>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::U32 => {
                build_output_stream::<u32>(&device, &config, mixer, start_frame)
            }
            cpal::SampleFormat::U64 => {
                build_output_stream::<u64>(&device, &config, mixer, start_frame)
            }
            format => return Err(format!("unsupported audio output sample format {format:?}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        Ok(Self { _stream: stream })
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: AudioMixer,
    start_frame: u64,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let output_channels = usize::from(config.channels);
    let output_rate = u64::from(config.sample_rate.0);
    let mut source_frame = start_frame;
    let mut source_remainder = 0_u64;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                for output_frame in output.chunks_mut(output_channels) {
                    let frame_samples = mixer.mix_frame_at(timeline_seconds_at_frame(source_frame));
                    let left = frame_samples.first().copied().unwrap_or(0.0);
                    let right = frame_samples.get(1).copied().unwrap_or(0.0);
                    for (channel, sample) in output_frame.iter_mut().enumerate() {
                        let value = if output_channels == 1 {
                            (left + right) * 0.5
                        } else if channel.is_multiple_of(2) {
                            left
                        } else {
                            right
                        };
                        *sample = T::from_sample(value);
                    }
                    source_remainder = source_remainder.saturating_add(u64::from(MIX_SAMPLE_RATE));
                    source_frame = source_frame.saturating_add(
                        source_remainder
                            .checked_div(output_rate)
                            .unwrap_or_default(),
                    );
                    source_remainder = source_remainder
                        .checked_rem(output_rate)
                        .unwrap_or_default();
                }
            },
            |error| eprintln!("audio playback stream failed: {error}"),
            None,
        )
        .map_err(|error| error.to_string())
}

fn timeline_seconds_at_frame(frame: u64) -> f64 {
    let rate = u64::from(MIX_SAMPLE_RATE);
    let seconds = frame.checked_div(rate).unwrap_or_default();
    let remainder = frame.checked_rem(rate).unwrap_or_default();
    let nanos = remainder
        .saturating_mul(1_000_000_000)
        .checked_div(rate)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    std::time::Duration::new(seconds, nanos).as_secs_f64()
}

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

    #[test]
    fn mixes_sources_with_offsets_and_gain() -> Result<(), AudioPacketError> {
        let first = AudioSource::new(vec![0.25; 8], 0.0, 1.0)?;
        let second = AudioSource::new(vec![0.5; 4], 2.0 / 48_000.0, 0.5)?;
        let mixer = AudioMixer::new(
            vec![first, second],
            vec![AudioTimelineSegment {
                source_start_secs: 0.0,
                source_end_secs: 4.0 / 48_000.0,
            }],
        );

        let packet = mixer.render(0.0, 4)?;
        assert_eq!(
            packet.samples(),
            &[0.25, 0.25, 0.25, 0.25, 0.5, 0.5, 0.5, 0.5]
        );
        Ok(())
    }

    #[test]
    fn follows_cut_timeline_segments() -> Result<(), AudioPacketError> {
        let samples = vec![
            0.0, 0.0, 0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4, 0.5, 0.5, 0.6, 0.6, 0.7, 0.7,
        ];
        let source = AudioSource::new(samples, 0.0, 1.0)?;
        let mixer = AudioMixer::new(
            vec![source],
            vec![
                AudioTimelineSegment {
                    source_start_secs: 0.0,
                    source_end_secs: 2.0 / 48_000.0,
                },
                AudioTimelineSegment {
                    source_start_secs: 6.0 / 48_000.0,
                    source_end_secs: 8.0 / 48_000.0,
                },
            ],
        );

        let packet = mixer.render(0.0, 4)?;
        assert_eq!(packet.samples(), &[0.0, 0.0, 0.1, 0.1, 0.6, 0.6, 0.7, 0.7]);
        Ok(())
    }
}
