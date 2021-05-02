use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::Method;
use serde::Deserialize;
use tokio::process::Command;

use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource, RetryableError, RetryableResult};
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Channel {
    id: String,
    name: String,
    is_external_tool: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Media {
    /* id: String, */
    name: String,
    last_updated_date: String,
    // used to download the stream
    stream_url_path: Option<String>, // Not all multimedia items are videos
}

pub struct MultimediaHandle {
    id: String,
    path: PathBuf,
}

pub struct Video {
    stream_url_path: String,
    path: PathBuf,
    last_updated: SystemTime,
}

impl MultimediaHandle {
    pub fn new(id: String, path: PathBuf) -> MultimediaHandle {
        MultimediaHandle { id, path }
    }

    // loads all (non-external) multimedia recursively
    // (I have no idea what external multimedia does, and I don't have a module to test it anyway)
    // it appears that there can't be nested directories for multimedia
    pub async fn load(self, api: &Api) -> Result<Vec<Video>> {
        let multimedia_resp = api
            .api_as_json::<ApiData<Vec<Channel>>>(
                &format!("multimedia/?ParentID={}", self.id),
                Method::GET,
                None,
            )
            .await?;

        match multimedia_resp.data {
            Some(channels) => future::join_all(
                channels
                    .into_iter()
                    .filter(|c| !c.is_external_tool)
                    .map(|c| Self::load_channel(api, c, &self.path)),
            )
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .map(|v| v.into_iter().flatten().collect::<Vec<_>>()),
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    async fn load_channel(api: &Api, channel: Channel, path: &Path) -> Result<Vec<Video>> {
        let channel_resp = api
            .api_as_json::<ApiData<Vec<Media>>>(
                &format!("multimedia/{}/medias", channel.id),
                Method::GET,
                None,
            )
            .await?;

        let channel_path = path.join(Path::new(&sanitise_filename(&channel.name)));

        match channel_resp.data {
            Some(medias) => Ok(medias
                .into_iter()
                .filter_map(|m| match m.stream_url_path {
                    Some(stream_url_path) => Some(Video {
                        stream_url_path: stream_url_path,
                        path: channel_path.join(Self::make_mkv_extension(Path::new(
                            &sanitise_filename(&m.name),
                        ))),
                        last_updated: parse_time(&m.last_updated_date),
                    }),
                    None => None,
                })
                .collect::<Vec<_>>()),
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    fn make_mkv_extension(path: &Path) -> PathBuf {
        path.with_extension("mkv")
    }
}

#[async_trait(?Send)]
impl Resource for Video {
    fn path(&self) -> &Path {
        &self.path
    }

    async fn download(
        &self,
        api: &Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult> {
        resource::do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.last_updated,
            move |_| future::ready(Ok(self.stream_url_path.as_str())),
            move |api, stream_url_path, temp_destination| {
                Self::stream_video(api, stream_url_path, temp_destination)
            },
        )
        .await
    }
}

impl Video {
    async fn stream_video(
        api: &Api,
        stream_url_path: &str,
        temp_destination: &Path,
    ) -> RetryableResult<()> {
        let success = Command::new(&api.ffmpeg_path)
            .arg("-y") // flag to overwrite output file without prompting
            .arg("-i")
            .arg(stream_url_path)
            .arg("-c")
            .arg("copy")
            .arg(temp_destination.as_os_str())
            .output()
            .await
            .map_err(|_| RetryableError::Fail("Failed to start ffmpeg"))?
            .status
            .success();
        if success {
            Ok(())
        } else {
            Err(RetryableError::Retry("ffmpeg returned nonzero exit code"))
        }
    }
}
