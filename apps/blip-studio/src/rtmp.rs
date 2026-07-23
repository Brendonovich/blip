use std::path::Path;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use async_channel::Sender;
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferLock_ReadOnly, kCVPixelFormatType_32BGRA,
};
use ffmpeg::{Dictionary, Packet, Rational, codec, encoder, format, frame, software};

const TIME_BASE: i32 = 90_000;
const FRAME_QUEUE_DEPTH: usize = 2;

#[derive(Clone)]
pub(crate) struct RtmpConfig {
    pub(crate) url: String,
    pub(crate) fps: u32,
    pub(crate) bitrate: usize,
}

pub(crate) struct RtmpStream {
    frames: Option<SyncSender<StreamFrame>>,
}

struct StreamFrame {
    image: SendablePixelBuffer,
}

struct SendablePixelBuffer(CVPixelBuffer);

// SAFETY: CVPixelBuffer is an explicitly shareable, retained Core Foundation type.
// This worker only reads the buffer after the compositor has finished writing it.
unsafe impl Send for SendablePixelBuffer {}

impl RtmpStream {
    pub(crate) fn start(
        config: RtmpConfig,
        dimensions: (usize, usize),
        generation: u64,
        error_sender: Sender<(u64, String)>,
    ) -> Result<Self> {
        validate_config(&config, dimensions)?;
        let (frame_sender, frame_receiver) = mpsc::sync_channel(FRAME_QUEUE_DEPTH);
        thread::Builder::new()
            .name("blip-rtmp-publisher".into())
            .spawn(move || {
                if let Err(error) = publish(&config, dimensions, &frame_receiver) {
                    let _ = error_sender.try_send((generation, error.to_string()));
                }
            })
            .context("failed to start RTMP publisher thread")?;
        Ok(Self {
            frames: Some(frame_sender),
        })
    }

    pub(crate) fn send(&self, image: CVPixelBuffer) -> Result<()> {
        let sender = self
            .frames
            .as_ref()
            .ok_or_else(|| anyhow!("RTMP publisher is stopping"))?;
        match sender.try_send(StreamFrame {
            image: SendablePixelBuffer(image),
        }) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => bail!("RTMP publisher stopped"),
        }
    }

    pub(crate) fn stop(&mut self) {
        self.frames.take();
    }
}

impl Drop for RtmpStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_config(config: &RtmpConfig, (width, height): (usize, usize)) -> Result<()> {
    if !config.url.starts_with("rtmp://") {
        bail!("RTMP destination must start with rtmp://");
    }
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        bail!("stream canvas dimensions must be non-zero and even");
    }
    if config.fps == 0 {
        bail!("stream frame rate must be greater than zero");
    }
    if config.bitrate == 0 {
        bail!("stream bitrate must be greater than zero");
    }
    Ok(())
}

fn publish(
    config: &RtmpConfig,
    dimensions: (usize, usize),
    frames: &mpsc::Receiver<StreamFrame>,
) -> Result<()> {
    ffmpeg::init().context("failed to initialize FFmpeg")?;
    format::network::init();
    let (width, height) = dimensions;
    let width = u32::try_from(width).context("canvas width exceeds FFmpeg limits")?;
    let height = u32::try_from(height).context("canvas height exceeds FFmpeg limits")?;
    let fps = i32::try_from(config.fps).context("frame rate exceeds FFmpeg limits")?;
    let time_base = Rational(1, TIME_BASE);

    let mut output = format::output_as(Path::new(&config.url), "flv")
        .context("failed to connect to RTMP destination")?;
    let codec = encoder::find_by_name("h264_videotoolbox")
        .ok_or_else(|| anyhow!("FFmpeg does not provide h264_videotoolbox"))?;
    let mut video = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .context("failed to create H.264 encoder")?;
    video.set_width(width);
    video.set_height(height);
    video.set_format(format::Pixel::NV12);
    video.set_time_base(time_base);
    video.set_frame_rate(Some(Rational(fps, 1)));
    video.set_bit_rate(
        config
            .bitrate
            .checked_mul(1_000)
            .ok_or_else(|| anyhow!("stream bitrate is too large"))?,
    );
    video.set_gop(config.fps.saturating_mul(2));
    video.set_max_b_frames(0);
    if output
        .format()
        .flags()
        .contains(format::Flags::GLOBAL_HEADER)
    {
        video.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut options = Dictionary::new();
    options.set("realtime", "true");
    options.set("prio_speed", "true");
    options.set("profile", "main");
    options.set("allow_sw", "false");
    let mut encoder = video
        .open_as_with(codec, options)
        .context("failed to open VideoToolbox H.264 encoder")?;
    let stream_index = {
        let mut stream = output
            .add_stream(codec)
            .context("failed to add the RTMP video stream")?;
        stream.set_time_base(time_base);
        stream.set_rate(Rational(fps, 1));
        stream.set_avg_frame_rate(Rational(fps, 1));
        stream.set_parameters(&encoder);
        stream.index()
    };
    output
        .write_header()
        .context("RTMP destination rejected the stream")?;

    let mut scaler = software::scaling::Context::get(
        format::Pixel::BGRA,
        width,
        height,
        format::Pixel::NV12,
        width,
        height,
        software::scaling::flag::Flags::FAST_BILINEAR,
    )
    .context("failed to create the BGRA to NV12 converter")?;
    let mut bgra = frame::Video::new(format::Pixel::BGRA, width, height);
    let mut nv12 = frame::Video::new(format::Pixel::NV12, width, height);
    let started_at = Instant::now();

    while let Ok(stream_frame) = frames.recv() {
        copy_pixel_buffer(&stream_frame.image.0, &mut bgra)?;
        scaler
            .run(&bgra, &mut nv12)
            .context("failed to convert a stream frame to NV12")?;
        nv12.set_pts(Some(timestamp_pts(started_at.elapsed())?));
        encoder
            .send_frame(&nv12)
            .context("failed to send a frame to VideoToolbox")?;
        drain_packets(&mut encoder, &mut output, stream_index, config.fps)?;
    }

    encoder.send_eof().context("failed to flush VideoToolbox")?;
    drain_packets(&mut encoder, &mut output, stream_index, config.fps)?;
    output
        .write_trailer()
        .context("failed to finish the RTMP stream")?;
    Ok(())
}

fn drain_packets(
    encoder: &mut ffmpeg::encoder::Video,
    output: &mut format::context::Output,
    stream_index: usize,
    fps: u32,
) -> Result<()> {
    let encoder_time_base = encoder.time_base();
    let stream_time_base = output
        .stream(stream_index)
        .ok_or_else(|| anyhow!("RTMP video stream disappeared"))?
        .time_base();
    loop {
        let mut packet = Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {
                packet.set_stream(stream_index);
                packet.set_duration(
                    i64::from(TIME_BASE)
                        .checked_div(i64::from(fps))
                        .ok_or_else(|| anyhow!("invalid stream frame rate"))?,
                );
                packet.rescale_ts(encoder_time_base, stream_time_base);
                packet.set_position(-1);
                packet
                    .write_interleaved(output)
                    .context("failed to write video to RTMP destination")?;
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(error) => return Err(error).context("failed to receive H.264 data"),
        }
    }
}

fn timestamp_pts(timestamp: Duration) -> Result<i64> {
    let micros = i64::try_from(timestamp.as_micros()).context("stream timestamp is too large")?;
    micros
        .checked_mul(i64::from(TIME_BASE))
        .and_then(|value| value.checked_div(1_000_000))
        .ok_or_else(|| anyhow!("stream timestamp is too large"))
}

#[allow(clippy::arithmetic_side_effects)]
fn copy_pixel_buffer(source: &CVPixelBuffer, destination: &mut frame::Video) -> Result<()> {
    if source.get_pixel_format() != kCVPixelFormatType_32BGRA {
        bail!("compositor produced a non-BGRA frame");
    }
    if source.get_width() != usize::try_from(destination.width())?
        || source.get_height() != usize::try_from(destination.height())?
    {
        bail!("compositor frame dimensions changed while streaming");
    }
    let status = source.lock_base_address(kCVPixelBufferLock_ReadOnly);
    if status != 0 {
        bail!("failed to lock compositor frame ({status})");
    }
    let result = (|| {
        let row_bytes = source
            .get_width()
            .checked_mul(4)
            .ok_or_else(|| anyhow!("canvas row size is too large"))?;
        let source_stride = source.get_bytes_per_row();
        let source_len = source_stride
            .checked_mul(source.get_height())
            .ok_or_else(|| anyhow!("canvas buffer is too large"))?;
        // SAFETY: The pixel buffer is locked for reading and exposes at least
        // `bytes_per_row * height` bytes until it is unlocked below.
        let source_data = unsafe {
            let address = source.get_base_address().cast::<u8>();
            if address.is_null() {
                return Err(anyhow!("compositor frame has no base address"));
            }
            std::slice::from_raw_parts(address, source_len)
        };
        let destination_stride = destination.stride(0);
        let destination_data = destination.data_mut(0);
        for row in 0..source.get_height() {
            let source_start = row * source_stride;
            let destination_start = row * destination_stride;
            let source_row = source_data
                .get(source_start..source_start + row_bytes)
                .ok_or_else(|| anyhow!("compositor frame row is out of bounds"))?;
            let destination_row = destination_data
                .get_mut(destination_start..destination_start + row_bytes)
                .ok_or_else(|| anyhow!("FFmpeg frame row is out of bounds"))?;
            destination_row.copy_from_slice(source_row);
        }
        Ok(())
    })();
    let unlock_status = source.unlock_base_address(kCVPixelBufferLock_ReadOnly);
    if unlock_status != 0 {
        bail!("failed to unlock compositor frame ({unlock_status})");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_rtmp_destinations() {
        let config = RtmpConfig {
            url: "https://example.com/live".into(),
            fps: 60,
            bitrate: 6_000,
        };
        assert!(validate_config(&config, (1920, 1080)).is_err());
    }

    #[test]
    fn converts_timestamp_to_ninety_kilohertz_time_base() {
        assert_eq!(timestamp_pts(Duration::from_millis(500)).ok(), Some(45_000));
    }
}
