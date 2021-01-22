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
struct Conference {
    id: String,
    name: String,
    start_date: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudRecord {
    record_instances: Option<Vec<CloudRecordInstance>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudRecordInstance {
    #[serde(rename = "shareURL")]
    share_url: String,
    password: String,
}

pub struct ConferencingHandle {
    id: String,
    path: PathBuf,
}

pub struct ZoomRecording {
    path: PathBuf,
    share_url: String,
    password: String,
    start_date: SystemTime,
}

impl ConferencingHandle {
    pub fn new(id: String, path: PathBuf) -> ConferencingHandle {
        ConferencingHandle { id, path }
    }

    // loads all (non-external) multimedia recursively
    // (I have no idea what external multimedia does, and I don't have a module to test it anyway)
    // it appears that there can't be nested directories for multimedia
    pub async fn load(self, api: &Api) -> Result<Vec<ZoomRecording>> {
        let conferencing_resp = api
            .api_as_json::<ApiData<Vec<Conference>>>(
                &format!(
                    "zoom/Meeting/{}/Meetings?offset=0&sortby=startDate%20asc&populate=null",
                    self.id
                ),
                Method::GET,
                None,
            )
            .await?;

        // Unfortunately, we can't tell if a recording is available from just the conference_resp,
        // so we have to poll each conference individually.
        // We also poll future meetings, since the meeting time is just a guideline anyway.
        match conferencing_resp.data {
            Some(conferences) => future::join_all(
                conferences
                    .into_iter()
                    .map(|c| load_cloud_record(api, c, &self.path)),
            )
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .map(|v| v.into_iter().flatten().collect::<Vec<_>>()),
            None => Err("Invalid API response from server: type mismatch"),
        }
    }
}

async fn load_cloud_record(
    api: &Api,
    conference: Conference,
    path: &Path,
) -> Result<Vec<ZoomRecording>> {
    let cloud_record = api
        .api_as_json::<CloudRecord>(
            &format!("zoom/Meeting/{}/cloudrecord", conference.id),
            Method::GET,
            None,
        )
        .await?;

    let start_date = parse_time(&conference.start_date);

    match cloud_record.record_instances {
        Some(record_instances) => Ok(match record_instances.len() {
            0 => vec![],
            1 => record_instances
                .into_iter()
                .map(|cri| ZoomRecording {
                    path: path.join(make_mp4_extension(Path::new(&sanitise_filename(
                        &conference.name,
                    )))),
                    share_url: cri.share_url,
                    password: cri.password,
                    start_date: start_date,
                })
                .collect::<Vec<_>>(),
            _ => record_instances
                .into_iter()
                .enumerate()
                .map(|(i, cri)| ZoomRecording {
                    path: path.join(make_mp4_extension(Path::new(&append_number(
                        &sanitise_filename(&conference.name),
                        i + 1,
                    )))),
                    share_url: cri.share_url,
                    password: cri.password,
                    start_date: start_date,
                })
                .collect::<Vec<_>>(),
        }),
        None => Ok(vec![]), // no recording for this meeting (maybe the recording hasn't been uploaded yet)
    }
}

fn make_mp4_extension(path: &Path) -> PathBuf {
    path.with_extension("mp4")
}

fn append_number(text: &str, number: usize) -> String {
    format!("{} ({})", text, number)
}

#[async_trait(?Send)]
impl Resource for ZoomRecording {
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
        unimplemented!();
        /*resource::do_retryable_download(
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
        .await*/
    }
}
