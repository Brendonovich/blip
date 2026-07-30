use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::time::Duration;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, kAudioDevicePropertyBufferFrameSize,
    kAudioDevicePropertyLatency, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreams,
    kAudioDevicePropertyTransportType, kAudioDeviceTransportTypeAirPlay,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    kAudioStreamPropertyDirection, kAudioStreamPropertyLatency,
};

const FALLBACK_SAMPLE_RATE: f64 = 48_000.0;
const BLUETOOTH_MIN_LATENCY: Duration = Duration::from_millis(120);
const BLUETOOTH_FALLBACK_LATENCY: Duration = Duration::from_millis(200);
const AIRPLAY_MIN_LATENCY: Duration = Duration::from_millis(1_800);
const MAX_OUTPUT_LATENCY: Duration = Duration::from_secs(3);

pub(crate) fn default_output_latency() -> Option<Duration> {
    let system = u32::try_from(kAudioObjectSystemObject).ok()?;
    let device = property::<AudioObjectID>(
        system,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
    )?;
    let transport = property::<u32>(
        device,
        kAudioDevicePropertyTransportType,
        kAudioObjectPropertyScopeGlobal,
    )
    .unwrap_or_default();
    let sample_rate = property::<f64>(
        device,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    )
    .filter(|rate| rate.is_finite() && *rate > 0.0)
    .unwrap_or(FALLBACK_SAMPLE_RATE);
    let device_latency = property::<u32>(
        device,
        kAudioDevicePropertyLatency,
        kAudioObjectPropertyScopeOutput,
    )
    .unwrap_or_default();
    let safety_offset = property::<u32>(
        device,
        kAudioDevicePropertySafetyOffset,
        kAudioObjectPropertyScopeOutput,
    )
    .unwrap_or_default();
    let buffer_frames = property::<u32>(
        device,
        kAudioDevicePropertyBufferFrameSize,
        kAudioObjectPropertyScopeGlobal,
    )
    .unwrap_or_default();
    let stream_latency = max_output_stream_latency(device).unwrap_or_default();
    let total_frames = u64::from(device_latency)
        .saturating_add(u64::from(safety_offset))
        .saturating_add(u64::from(buffer_frames))
        .saturating_add(u64::from(stream_latency));
    Some(latency_duration(total_frames, sample_rate, transport))
}

#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn latency_duration(total_frames: u64, sample_rate: f64, transport: u32) -> Duration {
    let wireless = transport == kAudioDeviceTransportTypeBluetooth
        || transport == kAudioDeviceTransportTypeBluetoothLE;
    let minimum = if transport == kAudioDeviceTransportTypeAirPlay {
        AIRPLAY_MIN_LATENCY
    } else if wireless {
        BLUETOOTH_MIN_LATENCY
    } else {
        Duration::ZERO
    };
    if total_frames == 0 {
        return if wireless {
            BLUETOOTH_FALLBACK_LATENCY
        } else {
            minimum
        };
    }
    Duration::try_from_secs_f64(total_frames as f64 / sample_rate)
        .unwrap_or_default()
        .max(minimum)
        .min(MAX_OUTPUT_LATENCY)
}

fn max_output_stream_latency(device: AudioObjectID) -> Option<u32> {
    let address = property_address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
    let mut byte_count = 0_u32;
    // SAFETY: The address and writable byte-count pointer are valid for this synchronous query.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            device,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut byte_count),
        )
    };
    if status != 0 || byte_count == 0 {
        return None;
    }
    let item_size = u32::try_from(size_of::<AudioObjectID>()).ok()?;
    let stream_count = byte_count.checked_div(item_size)?;
    let mut streams = vec![0_u32; usize::try_from(stream_count).ok()?];
    let output = NonNull::new(streams.as_mut_ptr())?.cast::<c_void>();
    // SAFETY: `output` has `byte_count` writable bytes and all query pointers are valid.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut byte_count),
            output,
        )
    };
    if status != 0 {
        return None;
    }
    streams
        .into_iter()
        .filter(|stream| {
            property::<u32>(
                *stream,
                kAudioStreamPropertyDirection,
                kAudioObjectPropertyScopeGlobal,
            ) == Some(0)
        })
        .filter_map(|stream| {
            property::<u32>(
                stream,
                kAudioStreamPropertyLatency,
                kAudioObjectPropertyScopeGlobal,
            )
        })
        .max()
}

fn property<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: u32,
) -> Option<T> {
    let address = property_address(selector, scope);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut byte_count = u32::try_from(size_of::<T>()).ok()?;
    let output = NonNull::new(value.as_mut_ptr())?.cast::<c_void>();
    // SAFETY: `output` has exactly `byte_count` writable bytes for this property type.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut byte_count),
            output,
        )
    };
    if status != 0 || usize::try_from(byte_count).ok()? != size_of::<T>() {
        return None;
    }
    // SAFETY: CoreAudio succeeded and initialized exactly `size_of::<T>()` bytes.
    Some(unsafe { value.assume_init() })
}

const fn property_address(
    selector: AudioObjectPropertySelector,
    scope: u32,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_bluetooth_floor_to_incomplete_hal_latency() {
        assert_eq!(
            latency_duration(256, 48_000.0, kAudioDeviceTransportTypeBluetooth),
            BLUETOOTH_MIN_LATENCY
        );
        assert_eq!(
            latency_duration(0, 48_000.0, kAudioDeviceTransportTypeBluetooth),
            BLUETOOTH_FALLBACK_LATENCY
        );
    }

    #[test]
    fn preserves_reported_latency_for_regular_outputs() {
        assert_eq!(
            latency_duration(4_800, 48_000.0, 0),
            Duration::from_millis(100)
        );
    }
}
