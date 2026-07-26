use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;

use reqwest::Client;
use reqwest::blocking::{Client as BlockingClient, Response as BlockingResponse};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::profiles::RecordingFormat;

const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateUpload<'a> {
    filename: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    format: UploadFormat,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum UploadFormat {
    Mp4,
    Hls,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadSession {
    id: String,
    upload_id: String,
    part_size: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignPart<'a> {
    upload_id: &'a str,
    part_number: usize,
}

#[derive(Deserialize)]
struct SignedPart {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignAsset<'a> {
    upload_id: &'a str,
    name: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedAsset {
    url: String,
    content_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteUpload<'a> {
    upload_id: &'a str,
    parts: &'a [UploadedPart],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadedPart {
    part_number: usize,
    etag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteResponse {
    viewer_url: String,
}

pub(crate) fn upload(
    path: &Path,
    configured_url: &str,
    format: RecordingFormat,
) -> Result<String, String> {
    let (base_url, token) = parse_server_url(configured_url)?;
    let client = BlockingClient::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("failed to create upload client: {error}"))?;
    let upload_path = if format == RecordingFormat::Hls {
        path.join("playlist.m3u8")
    } else {
        path.to_owned()
    };
    let size = upload_path
        .metadata()
        .map_err(|error| format!("failed to read recording metadata: {error}"))?
        .len();
    let filename = upload_filename(path, format);
    let uploads_url = endpoint(&base_url, "api/uploads")?;
    let session: UploadSession = decode(
        client
            .post(uploads_url)
            .bearer_auth(&token)
            .json(&CreateUpload {
                filename: &filename,
                size: Some(size),
                format: if format == RecordingFormat::Hls {
                    UploadFormat::Hls
                } else {
                    UploadFormat::Mp4
                },
            })
            .send(),
        "start upload",
    )?;
    if session.part_size < 5 * 1024 * 1024 {
        return Err("server returned an invalid multipart chunk size".into());
    }
    tracing::info!(
        upload_id = %session.upload_id,
        session_id = %session.id,
        format = ?format,
        total_bytes = size,
        "Started recording upload session"
    );

    let result = (if format == RecordingFormat::Hls {
        upload_hls_assets(&client, &base_url, &token, path, &session)
    } else {
        Ok(())
    })
    .and_then(|()| upload_parts(&client, &base_url, &token, &upload_path, &session))
    .and_then(|parts| {
        let complete_url = endpoint(&base_url, &format!("api/uploads/{}/complete", session.id))?;
        let response: CompleteResponse = decode(
            client
                .post(complete_url)
                .bearer_auth(&token)
                .json(&CompleteUpload {
                    upload_id: &session.upload_id,
                    parts: &parts,
                })
                .send(),
            "complete upload",
        )?;
        Ok(response.viewer_url)
    });

    match &result {
        Ok(viewer_url) => {
            tracing::info!(
                upload_id = %session.upload_id,
                viewer_url = %viewer_url,
                "Recording upload completed successfully"
            );
        }
        Err(error) => {
            tracing::error!(
                upload_id = %session.upload_id,
                error = %error,
                "Recording upload failed, aborting session"
            );
            let _ = client
                .delete(endpoint(&base_url, &format!("api/uploads/{}", session.id))?)
                .bearer_auth(&token)
                .json(&serde_json::json!({ "uploadId": session.upload_id }))
                .send();
        }
    }
    result
}

#[derive(Clone)]
pub(crate) struct HlsUpload {
    client: Client,
    base_url: Url,
    token: String,
    session: UploadSession,
    found_initialization: bool,
}

impl HlsUpload {
    pub(crate) async fn start(path: &Path, configured_url: &str) -> Result<Self, String> {
        let (base_url, token) = parse_server_url(configured_url)?;
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| format!("failed to create upload client: {error}"))?;
        let uploads_url = endpoint(&base_url, "api/uploads")?;
        let session: UploadSession = decode_async(
            client
                .post(uploads_url)
                .bearer_auth(&token)
                .json(&CreateUpload {
                    filename: &upload_filename(path, RecordingFormat::Hls),
                    size: None,
                    format: UploadFormat::Hls,
                })
                .send()
                .await,
            "start upload",
        )
        .await?;
        if session.part_size < 5 * 1024 * 1024 {
            return Err("server returned an invalid multipart chunk size".into());
        }
        tracing::info!(
            upload_id = %session.upload_id,
            session_id = %session.id,
            format = ?RecordingFormat::Hls,
            "Started incremental recording upload session"
        );
        Ok(Self {
            client,
            base_url,
            token,
            session,
            found_initialization: false,
        })
    }

    pub(crate) fn register_asset(&mut self, path: &Path) -> Result<(), String> {
        if validate_hls_asset(path)? {
            self.found_initialization = true;
        }
        Ok(())
    }

    pub(crate) async fn upload_asset(&self, path: &Path) -> Result<(), String> {
        validate_hls_asset(path)?;
        upload_hls_asset_async(
            &self.client,
            &self.base_url,
            &self.token,
            path,
            &self.session,
        )
        .await
    }

    pub(crate) async fn finish(&mut self, playlist: &Path) -> Result<String, String> {
        if !self.found_initialization {
            return Err("HLS recording is missing init.mp4".into());
        }
        let parts = upload_parts_async(
            &self.client,
            &self.base_url,
            &self.token,
            playlist,
            &self.session,
        )
        .await?;
        let complete_url = endpoint(
            &self.base_url,
            &format!("api/uploads/{}/complete", self.session.id),
        )?;
        let response: CompleteResponse = decode_async(
            self.client
                .post(complete_url)
                .bearer_auth(&self.token)
                .json(&CompleteUpload {
                    upload_id: &self.session.upload_id,
                    parts: &parts,
                })
                .send()
                .await,
            "complete upload",
        )
        .await?;
        tracing::info!(
            upload_id = %self.session.upload_id,
            viewer_url = %response.viewer_url,
            "Incremental recording upload completed successfully"
        );
        Ok(response.viewer_url)
    }

    pub(crate) async fn abort(&self) {
        tracing::info!(upload_id = %self.session.upload_id, "Aborting incremental upload session");
        let Ok(url) = endpoint(&self.base_url, &format!("api/uploads/{}", self.session.id)) else {
            return;
        };
        let _ = self
            .client
            .delete(url)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "uploadId": self.session.upload_id }))
            .send()
            .await;
    }
}

fn validate_hls_asset(path: &Path) -> Result<bool, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "HLS recording contains an invalid filename".to_owned())?;
    if name == "init.mp4" {
        Ok(true)
    } else if !(name.starts_with("segment")
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("m4s")))
    {
        Err(format!("HLS recording contains an unexpected file: {name}"))
    } else {
        Ok(false)
    }
}

fn upload_filename(path: &Path, format: RecordingFormat) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if format == RecordingFormat::Hls {
                "recording.hls".into()
            } else {
                "recording.mp4".into()
            }
        })
}

fn upload_hls_assets(
    client: &BlockingClient,
    base_url: &Url,
    token: &str,
    path: &Path,
    session: &UploadSession,
) -> Result<(), String> {
    let mut assets = path
        .read_dir()
        .map_err(|error| format!("failed to read HLS recording: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read HLS recording: {error}"))?;
    assets.sort_by_key(std::fs::DirEntry::file_name);
    let mut found_initialization = false;
    for asset in assets {
        let name = asset.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "HLS recording contains an invalid filename".to_owned())?;
        if name == "playlist.m3u8" || name.starts_with('.') {
            continue;
        }
        if name == "init.mp4" {
            found_initialization = true;
        } else if !(name.starts_with("segment")
            && std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("m4s")))
        {
            return Err(format!("HLS recording contains an unexpected file: {name}"));
        }
        upload_hls_asset(client, base_url, token, &asset.path(), session)?;
    }
    if !found_initialization {
        return Err("HLS recording is missing init.mp4".into());
    }
    Ok(())
}

fn upload_hls_asset(
    client: &BlockingClient,
    base_url: &Url,
    token: &str,
    path: &Path,
    session: &UploadSession,
) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "HLS recording contains an invalid filename".to_owned())?;
    let sign_url = endpoint(base_url, &format!("api/uploads/{}/assets", session.id))?;
    let signed: SignedAsset = decode(
        client
            .post(sign_url)
            .bearer_auth(token)
            .json(&SignAsset {
                upload_id: &session.upload_id,
                name,
            })
            .send(),
        "sign HLS asset",
    )?;
    let contents =
        std::fs::read(path).map_err(|error| format!("failed to read HLS asset {name}: {error}"))?;
    tracing::info!(
        asset_name = %name,
        asset_bytes = contents.len(),
        "Uploading HLS asset"
    );
    client
        .put(signed.url)
        .header(reqwest::header::CONTENT_TYPE, signed.content_type)
        .body(contents)
        .send()
        .and_then(BlockingResponse::error_for_status)
        .map_err(|error| format!("failed to upload HLS asset {name}: {error}"))?;
    tracing::debug!(asset_name = %name, "Uploaded HLS asset successfully");
    Ok(())
}

async fn upload_hls_asset_async(
    client: &Client,
    base_url: &Url,
    token: &str,
    path: &Path,
    session: &UploadSession,
) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "HLS recording contains an invalid filename".to_owned())?;
    let sign_url = endpoint(base_url, &format!("api/uploads/{}/assets", session.id))?;
    let signed: SignedAsset = decode_async(
        client
            .post(sign_url)
            .bearer_auth(token)
            .json(&SignAsset {
                upload_id: &session.upload_id,
                name,
            })
            .send()
            .await,
        "sign HLS asset",
    )
    .await?;
    let contents =
        std::fs::read(path).map_err(|error| format!("failed to read HLS asset {name}: {error}"))?;
    tracing::info!(
        asset_name = %name,
        asset_bytes = contents.len(),
        "Uploading HLS asset"
    );
    client
        .put(signed.url)
        .header(reqwest::header::CONTENT_TYPE, signed.content_type)
        .body(contents)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("failed to upload HLS asset {name}: {error}"))?;
    tracing::debug!(asset_name = %name, "Uploaded HLS asset successfully");
    Ok(())
}

fn upload_parts(
    client: &BlockingClient,
    base_url: &Url,
    token: &str,
    path: &Path,
    session: &UploadSession,
) -> Result<Vec<UploadedPart>, String> {
    let file = File::open(path).map_err(|error| format!("failed to open recording: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut parts = Vec::new();
    loop {
        let mut chunk = vec![0; session.part_size];
        let mut length = 0;
        while length < chunk.len() {
            let remaining = chunk
                .get_mut(length..)
                .ok_or_else(|| "invalid recording chunk offset".to_owned())?;
            let read = reader
                .read(remaining)
                .map_err(|error| format!("failed to read recording: {error}"))?;
            if read == 0 {
                break;
            }
            length = length
                .checked_add(read)
                .ok_or_else(|| "recording chunk size overflowed".to_owned())?;
        }
        if length == 0 {
            break;
        }
        chunk.truncate(length);
        let part_number = parts
            .len()
            .checked_add(1)
            .ok_or_else(|| "recording has too many chunks".to_owned())?;
        let sign_url = endpoint(base_url, &format!("api/uploads/{}/parts", session.id))?;
        let signed: SignedPart = decode(
            client
                .post(sign_url)
                .bearer_auth(token)
                .json(&SignPart {
                    upload_id: &session.upload_id,
                    part_number,
                })
                .send(),
            "sign upload chunk",
        )?;
        tracing::info!(
            part_number = part_number,
            chunk_bytes = length,
            "Uploading recording part"
        );
        let response = client
            .put(signed.url)
            .body(chunk)
            .send()
            .and_then(BlockingResponse::error_for_status)
            .map_err(|error| format!("failed to upload chunk {part_number}: {error}"))?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| format!("storage did not return an ETag for chunk {part_number}"))?
            .trim_matches('"')
            .to_owned();
        tracing::debug!(
            part_number = part_number,
            etag = %etag,
            "Uploaded recording part successfully"
        );
        parts.push(UploadedPart { part_number, etag });
    }
    if parts.is_empty() {
        return Err("cannot upload an empty recording".into());
    }
    Ok(parts)
}

async fn upload_parts_async(
    client: &Client,
    base_url: &Url,
    token: &str,
    path: &Path,
    session: &UploadSession,
) -> Result<Vec<UploadedPart>, String> {
    let file = File::open(path).map_err(|error| format!("failed to open recording: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut parts = Vec::new();
    loop {
        let mut chunk = vec![0; session.part_size];
        let mut length = 0;
        while length < chunk.len() {
            let remaining = chunk
                .get_mut(length..)
                .ok_or_else(|| "invalid recording chunk offset".to_owned())?;
            let read = reader
                .read(remaining)
                .map_err(|error| format!("failed to read recording: {error}"))?;
            if read == 0 {
                break;
            }
            length = length
                .checked_add(read)
                .ok_or_else(|| "recording chunk size overflowed".to_owned())?;
        }
        if length == 0 {
            break;
        }
        chunk.truncate(length);
        let part_number = parts
            .len()
            .checked_add(1)
            .ok_or_else(|| "recording has too many chunks".to_owned())?;
        let sign_url = endpoint(base_url, &format!("api/uploads/{}/parts", session.id))?;
        let signed: SignedPart = decode_async(
            client
                .post(sign_url)
                .bearer_auth(token)
                .json(&SignPart {
                    upload_id: &session.upload_id,
                    part_number,
                })
                .send()
                .await,
            "sign upload chunk",
        )
        .await?;
        tracing::info!(
            part_number,
            chunk_bytes = length,
            "Uploading recording part"
        );
        let response = client
            .put(signed.url)
            .body(chunk)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("failed to upload chunk {part_number}: {error}"))?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| format!("storage did not return an ETag for chunk {part_number}"))?
            .trim_matches('"')
            .to_owned();
        tracing::debug!(
            part_number,
            etag = %etag,
            "Uploaded recording part successfully"
        );
        parts.push(UploadedPart { part_number, etag });
    }
    if parts.is_empty() {
        return Err("cannot upload an empty recording".into());
    }
    Ok(parts)
}

fn parse_server_url(value: &str) -> Result<(Url, String), String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "invalid Blip server URL".to_owned())?;
    let token = url
        .fragment()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Blip server URL is missing its access token".to_owned())?
        .to_owned();
    url.set_fragment(None);
    if url.scheme() != "https" && !(url.scheme() == "http" && url.host_str() == Some("localhost")) {
        return Err("Blip server URL must use HTTPS".into());
    }
    Ok((url, token))
}

fn endpoint(base_url: &Url, path: &str) -> Result<Url, String> {
    let mut base = base_url.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(path)
        .map_err(|_| "invalid Blip server endpoint".to_owned())
}

fn decode<T: for<'de> Deserialize<'de>>(
    response: Result<BlockingResponse, reqwest::Error>,
    action: &str,
) -> Result<T, String> {
    let response = response.map_err(|error| format!("failed to {action}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body: String = response
            .text()
            .unwrap_or_default()
            .chars()
            .take(1_000)
            .collect();
        let detail = if body.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", body.trim())
        };
        return Err(format!("failed to {action}: HTTP status {status}{detail}"));
    }
    response.json().map_err(|error| {
        format!("server returned an invalid response while trying to {action}: {error}")
    })
}

async fn decode_async<T: for<'de> Deserialize<'de>>(
    response: Result<reqwest::Response, reqwest::Error>,
    action: &str,
) -> Result<T, String> {
    let response = response.map_err(|error| format!("failed to {action}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body: String = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(1_000)
            .collect();
        let detail = if body.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", body.trim())
        };
        return Err(format!("failed to {action}: HTTP status {status}{detail}"));
    }
    response.json().await.map_err(|error| {
        format!("server returned an invalid response while trying to {action}: {error}")
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{endpoint, parse_server_url, upload_filename};
    use crate::profiles::RecordingFormat;

    #[test]
    fn separates_fragment_token_from_server_url() -> Result<(), String> {
        let (url, token) = parse_server_url("https://blip.example/base#secret")?;
        assert_eq!(url.as_str(), "https://blip.example/base");
        assert_eq!(token, "secret");
        assert_eq!(
            endpoint(&url, "api/uploads")?.as_str(),
            "https://blip.example/base/api/uploads"
        );
        Ok(())
    }

    #[test]
    fn requires_a_secure_url_and_token() {
        assert!(parse_server_url("https://blip.example").is_err());
        assert!(parse_server_url("http://blip.example#secret").is_err());
        assert!(parse_server_url("http://localhost:8787#secret").is_ok());
    }

    #[test]
    fn uploads_recording_name_for_all_formats() {
        assert_eq!(
            upload_filename(
                Path::new("/tmp/Safari - Docs - 2026-07-26.mp4"),
                RecordingFormat::Mp4
            ),
            "Safari - Docs - 2026-07-26.mp4"
        );
        assert_eq!(
            upload_filename(
                Path::new("/tmp/Built-in Display - 2026-07-26.hls"),
                RecordingFormat::Hls
            ),
            "Built-in Display - 2026-07-26.hls"
        );
    }
}
