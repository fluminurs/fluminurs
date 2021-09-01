use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::Method;
use serde::Deserialize;

use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource};
use crate::streamer::stream_video;
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

mod external_multimedia;
pub use external_multimedia::ExternalVideo;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub is_external_tool: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InternalMedia {
    id: String,
    name: String,
    last_updated_date: String,
    // used to download the stream
    stream_url_path: Option<String>, // Not all multimedia items are videos
}

pub struct MultimediaHandle {
    id: String,
    path: PathBuf,
}

pub struct InternalVideo {
    id: String,
    stream_url_path: String,
    path: PathBuf,
    last_updated: SystemTime,
}

enum VideoList {
    Internal(Vec<InternalVideo>),
    External(Vec<ExternalVideo>),
}

impl MultimediaHandle {
    pub fn new(id: String, path: PathBuf) -> MultimediaHandle {
        MultimediaHandle { id, path }
    }

    // it appears that there can't be nested directories for multimedia
    pub async fn load(self, api: &Api) -> Result<(Vec<InternalVideo>, Vec<ExternalVideo>)> {
        let multimedia_resp = api
            .api_as_json::<ApiData<Vec<Channel>>>(
                &format!("multimedia/?populate=contentSummary&ParentID={}", self.id),
                Method::GET,
                None,
            )
            .await?;

        match multimedia_resp.data {
            Some(channels) => {
                let video_lists: Vec<Result<VideoList>> =
                    future::join_all(channels.into_iter().map(|c| async {
                        Ok(if !c.is_external_tool {
                            VideoList::Internal(Self::load_channel(api, c, &self.path).await?)
                        } else {
                            VideoList::External(
                                external_multimedia::load_external_channel(api, c, &self.path)
                                    .await?,
                            )
                        })
                    }))
                    .await;
                let mut internal_videos = Vec::new();
                let mut external_videos = Vec::new();
                for video_list in video_lists {
                    match video_list? {
                        VideoList::Internal(mut iv) => internal_videos.append(&mut iv),
                        VideoList::External(mut ev) => external_videos.append(&mut ev),
                    }
                }
                Ok((internal_videos, external_videos))
            }
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    async fn load_channel(api: &Api, channel: Channel, path: &Path) -> Result<Vec<InternalVideo>> {
        let channel_resp = api
            .api_as_json::<ApiData<Vec<InternalMedia>>>(
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
                    Some(stream_url_path) => Some(InternalVideo {
                        id: m.id,
                        stream_url_path,
                        path: channel_path
                            .join(make_mp4_extension(Path::new(&sanitise_filename(&m.name)))),
                        last_updated: parse_time(&m.last_updated_date),
                    }),
                    None => None,
                })
                .collect::<Vec<_>>()),
            None => Err("Invalid API response from server: type mismatch"),
        }
    }
}

// TODO: check file extension?
fn make_mp4_extension(path: &Path) -> PathBuf {
    path.with_extension("mp4")
}

#[async_trait(?Send)]
impl Resource for InternalVideo {
    fn id(&self) -> &str {
        &self.id
    }

    fn path(&self) -> &Path {
        &self.path
    }
    fn path_mut(&mut self) -> &mut PathBuf {
        &mut self.path
    }

    fn last_updated(&self) -> SystemTime {
        self.last_updated
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
            self.last_updated(),
            move |_| future::ready(Ok(self.stream_url_path.as_str())),
            move |api, stream_url_path, temp_destination| {
                stream_video(api, stream_url_path, temp_destination)
            },
        )
        .await
    }
}
