use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

mod assets;
mod compositor;
mod headless;
mod numeric_input;
mod rtmp;
mod theme;
mod viewer;

#[derive(Debug, Parser)]
#[command(
    name = "blip-studio",
    about = "Composite captured macOS video in real time"
)]
struct StreamArgs {
    /// Display ID reported by `blip-cli displays list`.
    #[arg(long, value_name = "ID", conflicts_with = "window")]
    display: Option<u32>,

    /// Window ID reported by `blip-cli windows list`.
    #[arg(long, value_name = "ID", conflicts_with = "display")]
    window: Option<u32>,

    /// Maximum requested frame rate.
    #[arg(long, default_value_t = 60)]
    fps: u32,

    /// Include the cursor in captured frames.
    #[arg(long)]
    cursor: bool,

    /// Target H.264 video bitrate in kilobits per second.
    #[arg(long, default_value_t = 6_000, value_name = "KBPS")]
    bitrate: usize,

    /// Run without a window using a serialized JSON scene graph.
    #[arg(long, requires = "scene")]
    headless: bool,

    /// Serialized JSON scene graph to load.
    #[arg(long, value_name = "PATH")]
    scene: Option<PathBuf>,

    /// Stop a headless run after this many seconds.
    #[arg(long, default_value_t = 10, value_name = "SECONDS")]
    duration: u64,

    /// Measure capture delivery without running the compositor.
    #[arg(long, requires = "headless")]
    capture_only: bool,
}

fn main() -> ExitCode {
    let args = StreamArgs::parse();
    let result = if args.headless {
        headless::run(&args)
    } else {
        viewer::view(&args)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("blip-studio: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_capture_options() {
        let result =
            StreamArgs::try_parse_from(["blip-studio", "--window", "1", "--fps", "30", "--cursor"]);
        assert!(result.is_ok(), "stream app should accept capture options");
    }

    #[test]
    fn accepts_stream_bitrate() {
        let result = StreamArgs::try_parse_from(["blip-studio", "--bitrate", "4500"]);
        assert!(result.is_ok(), "stream app should accept a stream bitrate");
    }

    #[test]
    fn rejects_removed_rtmp_url_option() {
        let result =
            StreamArgs::try_parse_from(["blip-studio", "--rtmp-url", "rtmp://localhost/live/test"]);
        assert!(
            result.is_err(),
            "RTMP destinations should be entered in the UI"
        );
    }

    #[test]
    fn rejects_display_with_window() {
        let result = StreamArgs::try_parse_from(["blip-studio", "--display", "1", "--window", "2"]);
        assert!(result.is_err(), "display and window should conflict");
    }

    #[test]
    fn accepts_headless_serialized_scene() {
        let result = StreamArgs::try_parse_from([
            "blip-studio",
            "--headless",
            "--scene",
            "scene.json",
            "--duration",
            "5",
            "--capture-only",
        ]);
        assert!(result.is_ok(), "stream app should accept a headless scene");
    }
}
