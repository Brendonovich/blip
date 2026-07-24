use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordingFormat {
    Mp4,
    Hls,
    BlipBundle,
}

impl RecordingFormat {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Hls => "HLS",
            Self::BlipBundle => "Blip Bundle",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RecordingTarget {
    Local { folder: PathBuf },
    Remote { server_url: String },
}

pub(crate) fn split_server_url(value: &str) -> (String, String) {
    let Ok(mut url) = url::Url::parse(value.trim()) else {
        return (value.trim().to_owned(), String::new());
    };
    let token = url.fragment().unwrap_or_default().to_owned();
    url.set_fragment(None);
    (url.into(), token)
}

pub(crate) fn join_server_url(server_url: &str, token: &str) -> String {
    let server_url = server_url.trim();
    let token = token.trim();
    let Ok(mut url) = url::Url::parse(server_url) else {
        return format!("{server_url}#{token}");
    };
    url.set_fragment(Some(token));
    url.into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionAction {
    Reveal,
    CopyToClipboard,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordingProfile {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) target: RecordingTarget,
    pub(crate) format: RecordingFormat,
    pub(crate) completion_action: CompletionAction,
}

impl RecordingProfile {
    pub(crate) fn local(
        id: impl Into<String>,
        name: impl Into<String>,
        folder: PathBuf,
        completion_action: CompletionAction,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            target: RecordingTarget::Local { folder },
            format: RecordingFormat::Mp4,
            completion_action,
        }
    }

    pub(crate) fn new_local(folder: PathBuf) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self::local(
            format!("profile-{nanos}"),
            "New profile",
            folder,
            CompletionAction::Reveal,
        )
    }

    fn new_remote(name: String, server_url: String) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self {
            id: format!("profile-{nanos}"),
            name,
            target: RecordingTarget::Remote { server_url },
            format: RecordingFormat::Mp4,
            completion_action: CompletionAction::None,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Profile name cannot be empty".into());
        }
        match &self.target {
            RecordingTarget::Local { folder } => {
                if self.format == RecordingFormat::Hls {
                    return Err("Local recordings must use MP4 or Blip Bundle".into());
                }
                if folder.as_os_str().is_empty() {
                    Err("Choose a local recording folder".into())
                } else {
                    Ok(())
                }
            }
            RecordingTarget::Remote { server_url } => {
                if self.format == RecordingFormat::BlipBundle {
                    return Err("Blip server recordings must use HLS or MP4".into());
                }
                let Ok(url) = url::Url::parse(server_url.trim()) else {
                    return Err("Enter a valid Blip server URL".into());
                };
                if url.scheme() != "https"
                    && !(url.scheme() == "http" && url.host_str() == Some("localhost"))
                {
                    Err("The Blip server URL must use HTTPS".into())
                } else if url.fragment().is_none_or(str::is_empty) {
                    Err("The Blip server URL must include its access token".into())
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RecordingProfiles {
    pub(crate) profiles: Vec<RecordingProfile>,
    pub(crate) selected_profile_id: String,
}

impl Default for RecordingProfiles {
    fn default() -> Self {
        let home = home_dir();
        Self {
            profiles: vec![
                RecordingProfile::local(
                    "desktop",
                    "Desktop",
                    home.join("Desktop"),
                    CompletionAction::Reveal,
                ),
                RecordingProfile::local(
                    "documents",
                    "Documents",
                    home.join("Documents"),
                    CompletionAction::Reveal,
                ),
                RecordingProfile::local(
                    "downloads",
                    "Downloads",
                    home.join("Downloads"),
                    CompletionAction::Reveal,
                ),
                RecordingProfile::local(
                    "clipboard",
                    "Clipboard",
                    std::env::temp_dir().join("blip-capture"),
                    CompletionAction::CopyToClipboard,
                ),
            ],
            selected_profile_id: "desktop".into(),
        }
    }
}

impl RecordingProfiles {
    pub(crate) fn load() -> Self {
        let path = settings_path();
        let Ok(contents) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut settings) = serde_json::from_str::<Self>(&contents) else {
            return Self::default();
        };
        if settings.profiles.is_empty() {
            return Self::default();
        }
        if !settings
            .profiles
            .iter()
            .any(|profile| profile.id == settings.selected_profile_id)
            && let Some(profile) = settings.profiles.first()
        {
            settings.selected_profile_id = profile.id.clone();
        }
        settings
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let path = settings_path();
        let parent = path
            .parent()
            .ok_or_else(|| "invalid profile settings path".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create settings folder: {error}"))?;
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| format!("failed to encode recording profiles: {error}"))?;
        fs::write(path, contents)
            .map_err(|error| format!("failed to save recording profiles: {error}"))
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.profiles
            .iter()
            .position(|profile| profile.id == self.selected_profile_id)
            .unwrap_or(0)
    }

    pub(crate) fn selected(&self) -> Option<&RecordingProfile> {
        self.profiles.get(self.selected_index())
    }

    pub(crate) fn import_url(&mut self, value: &str) -> Result<(), String> {
        let import_url = url::Url::parse(value).map_err(|_| "invalid Blip Capture URL")?;
        if import_url.scheme() != "blip-capture" || import_url.host_str() != Some("add-profile") {
            return Err("unsupported Blip Capture URL".into());
        }
        let server_url = import_url
            .query_pairs()
            .find_map(|(key, value)| (key == "url").then(|| value.into_owned()))
            .ok_or_else(|| "Blip Capture URL is missing its server URL".to_owned())?;

        let (base_url, _) = split_server_url(&server_url);
        if let Some(profile) = self.profiles.iter_mut().find(|profile| {
            matches!(&profile.target, RecordingTarget::Remote { server_url } if split_server_url(server_url).0 == base_url)
        }) {
            profile.target = RecordingTarget::Remote { server_url };
        self.selected_profile_id.clone_from(&profile.id);
            return Ok(());
        }

        let server = url::Url::parse(&server_url).map_err(|_| "invalid Blip server URL")?;
        let name = server.host_str().unwrap_or("Blip server").to_owned();
        let profile = RecordingProfile::new_remote(name, server_url);
        profile.validate()?;
        self.selected_profile_id.clone_from(&profile.id);
        self.profiles.push(profile);
        Ok(())
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn settings_path() -> PathBuf {
    home_dir()
        .join(Path::new("Library/Application Support/Blip Capture"))
        .join("recording-profiles.json")
}

#[cfg(test)]
mod tests {
    use super::{RecordingProfiles, RecordingTarget, join_server_url, split_server_url};

    #[test]
    fn defaults_preserve_existing_destinations() {
        let profiles = RecordingProfiles::default();
        assert_eq!(profiles.profiles.len(), 4);
        assert_eq!(
            profiles.selected().map(|profile| profile.name.as_str()),
            Some("Desktop")
        );
    }

    #[test]
    fn validates_target_configuration() {
        let mut profile = RecordingProfiles::default().profiles.remove(0);
        assert!(profile.validate().is_ok());
        profile.target = RecordingTarget::Remote {
            server_url: "https://media.example.com#secret".into(),
        };
        profile.format = super::RecordingFormat::Hls;
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn separates_profile_server_url_and_token() {
        assert_eq!(
            split_server_url("https://blip.example/base#blip_secret"),
            (
                "https://blip.example/base".to_owned(),
                "blip_secret".to_owned()
            )
        );
        assert_eq!(
            join_server_url("https://blip.example/base", "blip_secret"),
            "https://blip.example/base#blip_secret"
        );
    }

    #[test]
    fn imports_and_selects_remote_profiles() -> Result<(), String> {
        let mut profiles = RecordingProfiles::default();
        let import = "blip-capture://add-profile?url=https%3A%2F%2Fblip.example%23secret";
        profiles.import_url(import)?;

        assert_eq!(profiles.profiles.len(), 5);
        assert_eq!(
            profiles.selected().map(|profile| profile.name.as_str()),
            Some("blip.example")
        );
        assert!(matches!(
            profiles.selected().map(|profile| &profile.target),
            Some(RecordingTarget::Remote { server_url }) if server_url == "https://blip.example#secret"
        ));

        profiles.import_url(import)?;
        assert_eq!(profiles.profiles.len(), 5);

        profiles.import_url(
            "blip-capture://add-profile?url=https%3A%2F%2Fblip.example%23replacement",
        )?;
        assert_eq!(profiles.profiles.len(), 5);
        assert!(matches!(
            profiles.selected().map(|profile| &profile.target),
            Some(RecordingTarget::Remote { server_url }) if server_url == "https://blip.example#replacement"
        ));
        Ok(())
    }
}
