use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "blip-cli", about = "Capture macOS displays and windows")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Inspect displays available to `ScreenCaptureKit`.
    Displays {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Inspect windows available to `ScreenCaptureKit`.
    Windows {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Stream video frames until Ctrl-C.
    Stream(StreamArgs),
    /// Record captured video frames to an MP4 file.
    Record(RecordArgs),
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum ResourceCommand {
    /// List resources and their IDs.
    List,
    /// Show detailed information for a resource.
    Info {
        /// Display or window ID reported by the corresponding list command.
        id: u32,
    },
}

#[derive(Debug, Args)]
pub(crate) struct StreamArgs {
    /// Display ID reported by `blip-cli displays list`.
    #[arg(long, value_name = "ID", conflicts_with = "window")]
    pub(crate) display: Option<u32>,

    /// Window ID reported by `blip-cli windows list`.
    #[arg(long, value_name = "ID", conflicts_with = "display")]
    pub(crate) window: Option<u32>,

    /// Maximum requested frame rate.
    #[arg(long, default_value_t = 60)]
    pub(crate) fps: u32,

    /// Include the cursor in captured frames.
    #[arg(long)]
    pub(crate) cursor: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RecordArgs {
    #[command(flatten)]
    pub(crate) capture: StreamArgs,

    /// MP4 file to create. An existing file is replaced.
    #[arg(value_name = "OUTPUT")]
    pub(crate) output: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_fps_with_display() {
        let result = Cli::try_parse_from(["blip-cli", "stream", "--display", "1", "--fps", "30"]);
        assert!(result.is_ok(), "display and FPS should be compatible");
    }

    #[test]
    fn accepts_fps_with_window() {
        let result = Cli::try_parse_from(["blip-cli", "stream", "--window", "1", "--fps", "30"]);
        assert!(result.is_ok(), "window and FPS should be compatible");
    }

    #[test]
    fn rejects_display_with_window() {
        let result = Cli::try_parse_from(["blip-cli", "stream", "--display", "1", "--window", "2"]);
        assert!(result.is_err(), "display and window should conflict");
    }

    #[test]
    fn record_accepts_capture_options_and_output() {
        let result = Cli::try_parse_from([
            "blip-cli",
            "record",
            "capture.mp4",
            "--display",
            "1",
            "--fps",
            "30",
            "--cursor",
        ]);
        assert!(result.is_ok(), "record should accept capture options");
    }

    #[test]
    fn record_requires_output() {
        let result = Cli::try_parse_from(["blip-cli", "record", "--display", "1"]);
        assert!(result.is_err(), "record should require an output path");
    }

    #[test]
    fn record_rejects_display_with_window() {
        let result = Cli::try_parse_from([
            "blip-cli",
            "record",
            "capture.mp4",
            "--display",
            "1",
            "--window",
            "2",
        ]);
        assert!(result.is_err(), "display and window should conflict");
    }
}
