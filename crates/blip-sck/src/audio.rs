use std::ptr;
use std::slice;
use std::time::Duration;

use blip_audio::AudioPacket;
use blip_media_time::FrameTimestamp;
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer,
};

use crate::CaptureError;

pub(crate) fn audio_packet(
    sample_buffer: &CMSampleBuffer,
    timestamp: Option<FrameTimestamp>,
    output_latency: Duration,
) -> Result<AudioPacket, CaptureError> {
    let timestamp = compensate_timestamp(timestamp, output_latency);
    // SAFETY: The retained callback sample has immutable format metadata.
    let format = unsafe { sample_buffer.format_description() }.ok_or_else(|| {
        CaptureError::InvalidFrame("system audio sample has no format description".into())
    })?;
    // SAFETY: ScreenCaptureKit identifies this sample as audio media.
    let description = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&format) };
    // SAFETY: A non-null ASBD pointer is valid while its retained format description lives.
    let description = unsafe { description.as_ref() }.ok_or_else(|| {
        CaptureError::InvalidFrame("system audio sample has no stream description".into())
    })?;
    if description.mFormatID != kAudioFormatLinearPCM
        || description.mFormatFlags & kAudioFormatFlagIsFloat == 0
        || description.mFormatFlags & kAudioFormatFlagIsPacked == 0
        || description.mBitsPerChannel != 32
    {
        return Err(CaptureError::InvalidFrame(
            "system audio is not packed 32-bit floating-point PCM".into(),
        ));
    }
    if description.mFormatFlags & kAudioFormatFlagIsBigEndian != 0 {
        return Err(CaptureError::InvalidFrame(
            "big-endian system audio is unsupported".into(),
        ));
    }
    let sample_rate = sample_rate(description.mSampleRate)?;
    let channels = u16::try_from(description.mChannelsPerFrame).map_err(|_| {
        CaptureError::InvalidFrame("system audio channel count exceeds supported range".into())
    })?;
    // SAFETY: The retained callback sample has immutable sample metadata.
    let frame_count = usize::try_from(unsafe { sample_buffer.num_samples() })
        .map_err(|_| CaptureError::InvalidFrame("system audio frame count is invalid".into()))?;
    let buffers = audio_buffers(sample_buffer)?;
    let listed_channels = buffers
        .iter()
        .try_fold(0_u32, |total, buffer| {
            total.checked_add(u32::from(buffer.channels))
        })
        .ok_or_else(|| CaptureError::InvalidFrame("system audio channel count overflow".into()))?;
    if listed_channels != u32::from(channels) {
        return Err(CaptureError::InvalidFrame(
            "system audio buffers do not match the stream channel count".into(),
        ));
    }

    let mut channel_data = Vec::with_capacity(buffers.len());
    for buffer in buffers {
        channel_data.push((buffer.channels, buffer.bytes));
    }

    let mut samples = Vec::with_capacity(frame_count.saturating_mul(usize::from(channels)));
    for frame in 0..frame_count {
        for (buffer_channels, bytes) in &channel_data {
            let buffer_channels = usize::from(*buffer_channels);
            for channel in 0..buffer_channels {
                let sample_index = frame
                    .checked_mul(buffer_channels)
                    .and_then(|index| index.checked_add(channel))
                    .ok_or_else(|| {
                        CaptureError::InvalidFrame("system audio sample index overflow".into())
                    })?;
                let offset = sample_index.checked_mul(size_of::<f32>()).ok_or_else(|| {
                    CaptureError::InvalidFrame("system audio sample offset overflow".into())
                })?;
                let end = offset.checked_add(size_of::<f32>()).ok_or_else(|| {
                    CaptureError::InvalidFrame("system audio sample offset overflow".into())
                })?;
                let sample = bytes.get(offset..end).ok_or_else(|| {
                    CaptureError::InvalidFrame(
                        "system audio buffer is shorter than expected".into(),
                    )
                })?;
                samples.push(f32::from_ne_bytes(sample.try_into().map_err(|_| {
                    CaptureError::InvalidFrame("invalid system audio sample".into())
                })?));
            }
        }
    }
    AudioPacket::new(samples, sample_rate, channels, timestamp)
        .map_err(|error| CaptureError::InvalidFrame(error.to_string()))
}

fn compensate_timestamp(
    timestamp: Option<FrameTimestamp>,
    output_latency: Duration,
) -> Option<FrameTimestamp> {
    timestamp.and_then(|timestamp| {
        timestamp
            .duration_since_epoch()
            .checked_add(output_latency)
            .map(FrameTimestamp::from_duration_since_epoch)
    })
}

fn sample_rate(value: f64) -> Result<u32, CaptureError> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) || value.fract() != 0.0 {
        return Err(CaptureError::InvalidFrame(
            "system audio sample rate is invalid".into(),
        ));
    }
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    Ok(value as u32)
}

struct AudioBufferData {
    channels: u16,
    bytes: Vec<u8>,
}

fn audio_buffers(sample_buffer: &CMSampleBuffer) -> Result<Vec<AudioBufferData>, CaptureError> {
    let mut required_size = 0_usize;
    // SAFETY: Null outputs request the required AudioBufferList storage size only.
    let status = unsafe {
        sample_buffer.audio_buffer_list_with_retained_block_buffer(
            &raw mut required_size,
            ptr::null_mut(),
            0,
            None,
            None,
            0,
            ptr::null_mut(),
        )
    };
    if required_size < size_of::<AudioBufferList>() {
        return Err(CaptureError::InvalidFrame(format!(
            "failed to inspect system audio buffers ({status})"
        )));
    }
    let word_count = required_size.div_ceil(size_of::<usize>());
    let mut storage = vec![0_usize; word_count];
    let list = storage.as_mut_ptr().cast::<AudioBufferList>();
    let mut block_buffer = ptr::null_mut::<CMBlockBuffer>();
    // SAFETY: `storage` is aligned and large enough for the requested variable-length list;
    // CoreMedia initializes the list and returns a retained owner for its sample data.
    let status = unsafe {
        sample_buffer.audio_buffer_list_with_retained_block_buffer(
            ptr::null_mut(),
            list,
            required_size,
            None,
            None,
            0,
            &raw mut block_buffer,
        )
    };
    if status != 0 {
        return Err(CaptureError::InvalidFrame(format!(
            "failed to read system audio buffers ({status})"
        )));
    }
    let block_buffer = ptr::NonNull::new(block_buffer).ok_or_else(|| {
        CaptureError::InvalidFrame("system audio has no retained data buffer".into())
    })?;
    // SAFETY: CoreMedia returned this object at retain count one; it keeps list data alive below.
    let _block_buffer = unsafe { CFRetained::from_raw(block_buffer) };
    // SAFETY: CoreMedia initialized `list` in the aligned storage above.
    let buffer_count = usize::try_from(unsafe { (*list).mNumberBuffers }).map_err(|_| {
        CaptureError::InvalidFrame("system audio buffer count exceeds supported range".into())
    })?;
    let buffers_size = buffer_count
        .checked_mul(size_of::<AudioBuffer>())
        .and_then(|size| size.checked_add(std::mem::offset_of!(AudioBufferList, mBuffers)))
        .ok_or_else(|| CaptureError::InvalidFrame("system audio buffer list overflow".into()))?;
    if buffers_size > storage.len().saturating_mul(size_of::<usize>()) {
        return Err(CaptureError::InvalidFrame(
            "system audio buffer list is shorter than expected".into(),
        ));
    }
    // SAFETY: AudioBufferList stores `mNumberBuffers` contiguous entries from `mBuffers`.
    let buffers = unsafe {
        slice::from_raw_parts(
            ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>(),
            buffer_count,
        )
    };
    buffers
        .iter()
        .map(|buffer| {
            let channels = u16::try_from(buffer.mNumberChannels).map_err(|_| {
                CaptureError::InvalidFrame(
                    "system audio buffer channel count exceeds supported range".into(),
                )
            })?;
            let byte_len = usize::try_from(buffer.mDataByteSize).map_err(|_| {
                CaptureError::InvalidFrame(
                    "system audio buffer size exceeds supported range".into(),
                )
            })?;
            if buffer.mData.is_null() {
                return Err(CaptureError::InvalidFrame(
                    "system audio buffer has no sample data".into(),
                ));
            }
            // SAFETY: The retained block buffer keeps this AudioBuffer's data alive while copied.
            let bytes = unsafe { slice::from_raw_parts(buffer.mData.cast::<u8>(), byte_len) };
            Ok(AudioBufferData {
                channels,
                bytes: bytes.to_vec(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_audio_timestamp_by_output_latency() {
        let timestamp = FrameTimestamp::from_duration_since_epoch(Duration::from_secs(10));
        assert_eq!(
            compensate_timestamp(Some(timestamp), Duration::from_millis(180)),
            Some(FrameTimestamp::from_duration_since_epoch(
                Duration::from_millis(10_180)
            ))
        );
    }
}
