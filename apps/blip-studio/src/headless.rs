use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use blip_avfoundation::{CameraCapturer, CameraDevice, CameraFrame};
use core_foundation::base::TCFType as _;
use core_video::pixel_buffer::CVPixelBuffer;
use serde::Deserialize;

use crate::StreamArgs;
use crate::compositor::{
    CompositorItem, CompositorItemContent, CompositorSource, FrameCompositor, ItemTransform,
};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
pub(crate) struct SceneGraph {
    #[serde(default = "default_canvas")]
    canvas: [usize; 2],
    sources: HashMap<String, SourceDefinition>,
    elements: Vec<ElementDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SourceDefinition {
    Camera {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        unique_id: Option<String>,
    },
    Color {
        rgb: [u8; 3],
    },
}

#[derive(Debug, Deserialize)]
struct ElementDefinition {
    source: String,
    #[serde(default = "default_center")]
    center: [f32; 2],
    #[serde(default = "default_size")]
    size: [f32; 2],
    #[serde(default)]
    corner_radius: f32,
}

struct TimedFrame {
    frame: CameraFrame,
    captured_at: Instant,
}

#[derive(Default)]
struct LatestFrames {
    frames: Mutex<HashMap<String, TimedFrame>>,
    ready: Condvar,
}

impl LatestFrames {
    fn submit(&self, source: String, frame: CameraFrame, metrics: &Metrics) {
        let presentation_timestamp = frame.presentation_timestamp();
        let mut frames = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if frames
            .insert(
                source,
                TimedFrame {
                    frame,
                    captured_at: Instant::now(),
                },
            )
            .is_some()
        {
            metrics.record_overwrite();
        }
        metrics.record_capture(presentation_timestamp);
        self.ready.notify_one();
    }

    fn wait_and_drain(&self, timeout: Duration) -> HashMap<String, TimedFrame> {
        let frames = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut frames = self
            .ready
            .wait_timeout_while(frames, timeout, |frames| frames.is_empty())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        std::mem::take(&mut *frames)
    }
}

#[derive(Default)]
struct Metrics {
    inner: Mutex<MetricValues>,
    last_presentation_timestamp: Mutex<Option<Duration>>,
}

#[derive(Default)]
struct MetricValues {
    captured: u64,
    dropped: u64,
    overwritten: u64,
    composed: u64,
    compose_time: Duration,
    pipeline_latency: Duration,
    maximum_compose_time: Duration,
    maximum_pipeline_latency: Duration,
    media_intervals: u64,
    media_duration: Duration,
}

impl Metrics {
    fn record_capture(&self, presentation_timestamp: Option<Duration>) {
        if let Ok(mut values) = self.inner.lock() {
            values.captured = values.captured.saturating_add(1);
            if let Some(timestamp) = presentation_timestamp
                && let Ok(mut previous) = self.last_presentation_timestamp.lock()
            {
                if let Some(previous_timestamp) = *previous
                    && let Some(interval) = timestamp.checked_sub(previous_timestamp)
                    && interval < Duration::from_secs(1)
                {
                    values.media_intervals = values.media_intervals.saturating_add(1);
                    values.media_duration = values.media_duration.saturating_add(interval);
                }
                *previous = Some(timestamp);
            }
        }
    }

    fn record_drop(&self) {
        if let Ok(mut values) = self.inner.lock() {
            values.dropped = values.dropped.saturating_add(1);
        }
    }

    fn record_overwrite(&self) {
        if let Ok(mut values) = self.inner.lock() {
            values.overwritten = values.overwritten.saturating_add(1);
        }
    }

    fn record_composition(&self, compose_time: Duration, pipeline_latency: Duration) {
        if let Ok(mut values) = self.inner.lock() {
            values.composed = values.composed.saturating_add(1);
            values.compose_time = values.compose_time.saturating_add(compose_time);
            values.pipeline_latency = values.pipeline_latency.saturating_add(pipeline_latency);
            values.maximum_compose_time = values.maximum_compose_time.max(compose_time);
            values.maximum_pipeline_latency = values.maximum_pipeline_latency.max(pipeline_latency);
        }
    }

    fn take(&self) -> MetricValues {
        self.inner.lock().map_or_else(
            |error| std::mem::take(&mut *error.into_inner()),
            |mut values| std::mem::take(&mut *values),
        )
    }
}

struct SourceFrame {
    pixel_buffer: CVPixelBuffer,
    captured_at: Instant,
}

/// Runs a serialized scene without creating a GPUI window.
///
/// # Errors
///
/// Returns an error when the scene is invalid or capture/composition fails.
pub(crate) fn run(args: &StreamArgs) -> Result<(), Box<dyn Error>> {
    let path = args
        .scene
        .as_deref()
        .ok_or("--scene is required with --headless")?;
    let graph = load_scene(path)?;
    validate_scene(&graph)?;
    let devices = blip_avfoundation::list_video_devices()?;
    let latest = Arc::new(LatestFrames::default());
    let metrics = Arc::new(Metrics::default());
    let mut capturers = Vec::new();

    for (id, source) in &graph.sources {
        let SourceDefinition::Camera { name, unique_id } = source else {
            continue;
        };
        let device = select_camera(&devices, name.as_deref(), unique_id.as_deref())?;
        let frame_latest = Arc::clone(&latest);
        let frame_metrics = Arc::clone(&metrics);
        let drop_metrics = Arc::clone(&metrics);
        let source_id = id.clone();
        let capturer = CameraCapturer::new_with_drop_callback(
            device,
            60,
            move |frame| frame_latest.submit(source_id.clone(), frame, &frame_metrics),
            move || drop_metrics.record_drop(),
        )?;
        if let Some(format) = capturer.capture_format() {
            let (width, height) = format.dimensions;
            eprintln!(
                "camera source={id} device=\"{}\" format={}x{}@{:.3} fourcc={}",
                device.localized_name(),
                width,
                height,
                format.frame_rate,
                fourcc(format.pixel_format),
            );
        }
        capturer.start()?;
        if let Some((minimum, maximum)) = capturer.active_frame_rate_range() {
            eprintln!("camera source={id} active_fps={minimum:.3}-{maximum:.3}");
        }
        capturers.push(capturer);
    }
    if capturers.is_empty() {
        return Err("headless scene contains no camera sources".into());
    }

    let result = run_pipeline(&graph, &latest, &metrics, args.capture_only, args.duration);
    for capturer in &capturers {
        capturer.stop();
    }
    result?;
    Ok(())
}

fn load_scene(path: &Path) -> anyhow::Result<SceneGraph> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read scene {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse scene {}", path.display()))
}

fn validate_scene(graph: &SceneGraph) -> anyhow::Result<()> {
    if graph.canvas.contains(&0) {
        return Err(anyhow!("canvas dimensions must be non-zero"));
    }
    for element in &graph.elements {
        if !graph.sources.contains_key(&element.source) {
            return Err(anyhow!(
                "scene element references unknown source {}",
                element.source
            ));
        }
        if element.size[0] <= 0.0 || element.size[1] <= 0.0 {
            return Err(anyhow!("scene element sizes must be positive"));
        }
    }
    Ok(())
}

fn select_camera<'a>(
    devices: &'a [CameraDevice],
    name: Option<&str>,
    unique_id: Option<&str>,
) -> anyhow::Result<&'a CameraDevice> {
    let device = devices.iter().find(|device| {
        unique_id.is_none_or(|id| device.unique_id() == id)
            && name.is_none_or(|name| device.localized_name().contains(name))
    });
    device.ok_or_else(|| {
        let available = devices
            .iter()
            .map(CameraDevice::localized_name)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow!("requested camera is unavailable; available cameras: {available}")
    })
}

fn run_pipeline(
    graph: &SceneGraph,
    latest: &LatestFrames,
    metrics: &Metrics,
    capture_only: bool,
    duration_seconds: u64,
) -> anyhow::Result<()> {
    set_interactive_thread_qos();
    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_secs(duration_seconds))
        .ok_or_else(|| anyhow!("headless duration exceeds the timer range"))?;
    let mut next_report = started
        .checked_add(REPORT_INTERVAL)
        .ok_or_else(|| anyhow!("report interval exceeds the timer range"))?;
    let mut last_report = started;
    let mut compositor = (!capture_only).then(FrameCompositor::new).transpose()?;
    let mut frames = HashMap::new();

    while Instant::now() < deadline {
        let drained = latest.wait_and_drain(Duration::from_millis(100));
        let received_frame = !drained.is_empty();
        for (source, timed) in drained {
            frames.insert(
                source,
                SourceFrame {
                    pixel_buffer: retain_camera_pixel_buffer(&timed.frame),
                    captured_at: timed.captured_at,
                },
            );
        }
        let now = Instant::now();
        if let Some(compositor) = &mut compositor
            && received_frame
            && !frames.is_empty()
        {
            compose(graph, compositor, &frames, metrics)?;
        }
        if now >= next_report {
            print_metrics(&metrics.take(), now.saturating_duration_since(last_report));
            last_report = now;
            next_report = now
                .checked_add(REPORT_INTERVAL)
                .ok_or_else(|| anyhow!("report interval exceeds the timer range"))?;
        }
    }
    print_metrics(
        &metrics.take(),
        Instant::now().saturating_duration_since(last_report),
    );
    Ok(())
}

fn set_interactive_thread_qos() {
    // SAFETY: This updates only the calling headless compositor thread's scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

fn compose(
    graph: &SceneGraph,
    compositor: &mut FrameCompositor,
    frames: &HashMap<String, SourceFrame>,
    metrics: &Metrics,
) -> anyhow::Result<()> {
    let mut source_indices = HashMap::new();
    let mut sources = Vec::new();
    for element in &graph.elements {
        if let Some(frame) = frames.get(&element.source)
            && !source_indices.contains_key(element.source.as_str())
        {
            source_indices.insert(element.source.as_str(), sources.len());
            sources.push(CompositorSource {
                pixel_buffer: &frame.pixel_buffer,
                content_rect: None,
            });
        }
    }
    let items = graph
        .elements
        .iter()
        .filter_map(|element| {
            let transform = ItemTransform::new(element.center, element.size)
                .with_corner_radius(element.corner_radius);
            match graph.sources.get(&element.source)? {
                SourceDefinition::Camera { .. } => Some(CompositorItem {
                    content: CompositorItemContent::Source(
                        *source_indices.get(element.source.as_str())?,
                    ),
                    transform,
                }),
                SourceDefinition::Color { rgb } => Some(CompositorItem {
                    content: CompositorItemContent::Color([
                        f32::from(rgb[0]) / 255.0,
                        f32::from(rgb[1]) / 255.0,
                        f32::from(rgb[2]) / 255.0,
                        1.0,
                    ]),
                    transform,
                }),
            }
        })
        .collect::<Vec<_>>();
    let newest_capture = graph
        .elements
        .iter()
        .filter_map(|element| frames.get(&element.source))
        .map(|frame| frame.captured_at)
        .max()
        .unwrap_or_else(Instant::now);
    let compose_started = Instant::now();
    let _output = compositor.render(&sources, &items, (graph.canvas[0], graph.canvas[1]))?;
    let completed = Instant::now();
    metrics.record_composition(
        completed.saturating_duration_since(compose_started),
        completed.saturating_duration_since(newest_capture),
    );
    Ok(())
}

fn print_metrics(values: &MetricValues, interval: Duration) {
    if values.captured == 0
        && values.dropped == 0
        && values.overwritten == 0
        && values.composed == 0
    {
        return;
    }
    let seconds = interval.as_secs_f64();
    let captured = u32::try_from(values.captured).unwrap_or(u32::MAX);
    let composed = u32::try_from(values.composed).unwrap_or(u32::MAX);
    let media_intervals = u32::try_from(values.media_intervals).unwrap_or(u32::MAX);
    let media_fps = if values.media_duration.is_zero() {
        0.0
    } else {
        f64::from(media_intervals) / values.media_duration.as_secs_f64()
    };
    let compose_average = average_duration(values.compose_time, values.composed);
    let latency_average = average_duration(values.pipeline_latency, values.composed);
    eprintln!(
        "timing capture={:.1}fps media={media_fps:.1}fps av_dropped={} overwritten={} render={:.1}fps compose_avg={:.2}ms compose_max={:.2}ms latency_avg={:.2}ms latency_max={:.2}ms",
        f64::from(captured) / seconds,
        values.dropped,
        values.overwritten,
        f64::from(composed) / seconds,
        compose_average.as_secs_f64() * 1000.0,
        values.maximum_compose_time.as_secs_f64() * 1000.0,
        latency_average.as_secs_f64() * 1000.0,
        values.maximum_pipeline_latency.as_secs_f64() * 1000.0,
    );
}

fn average_duration(total: Duration, count: u64) -> Duration {
    u32::try_from(count)
        .ok()
        .filter(|count| *count > 0)
        .and_then(|count| total.checked_div(count))
        .unwrap_or_default()
}

fn fourcc(value: u32) -> String {
    String::from_utf8_lossy(&value.to_be_bytes()).into_owned()
}

fn retain_camera_pixel_buffer(frame: &CameraFrame) -> CVPixelBuffer {
    let pixel_buffer = ptr::from_ref(frame.image_buffer())
        .cast_mut()
        .cast::<core_video::buffer::__CVBuffer>();
    // SAFETY: Both bindings represent the same retained CoreVideo pixel-buffer object.
    unsafe { CVPixelBuffer::wrap_under_get_rule(pixel_buffer) }
}

const fn default_canvas() -> [usize; 2] {
    [1920, 1080]
}

const fn default_center() -> [f32; 2] {
    [0.5, 0.5]
}

const fn default_size() -> [f32; 2] {
    [1.0, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_serialized_camera_scene() {
        let graph = serde_json::from_str::<SceneGraph>(
            r#"{
                "sources": {"camera": {"type": "camera", "name": "FaceTime"}},
                "elements": [{"source": "camera"}]
            }"#,
        );
        assert!(graph.is_ok());
        if let Ok(graph) = graph {
            assert!(validate_scene(&graph).is_ok());
            assert_eq!(graph.canvas, [1920, 1080]);
        }
    }

    #[test]
    fn rejects_unknown_scene_source() {
        let graph = SceneGraph {
            canvas: [1920, 1080],
            sources: HashMap::new(),
            elements: vec![ElementDefinition {
                source: "missing".into(),
                center: [0.5, 0.5],
                size: [1.0, 1.0],
                corner_radius: 0.0,
            }],
        };
        assert!(validate_scene(&graph).is_err());
    }
}
