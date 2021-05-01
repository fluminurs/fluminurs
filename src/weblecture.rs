use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::{Method, Url};
use serde::Deserialize;
use tokio::process::Command;

use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource, RetryableError, RetryableResult};
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebLectureResponse {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebLectureMedia {
    id: String,
    name: String,
    last_updated_date: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PanoptoRequestConstructionDetails {
    // TODO: rename?
    launchURL: String,
    data_items: Vec<PanoptoQueryParameter>
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PanoptoQueryParameter {
    key: String,
    value: String,
}

pub struct WebLectureHandle {
    id: String,
    path: PathBuf,
}

pub struct WebLectureVideo {
    // TODO: replace with URL?
    video_url: String,
    path: PathBuf,
    last_updated: SystemTime,
}

impl WebLectureHandle {
    pub fn new(id: String, path: PathBuf) -> WebLectureHandle {
        WebLectureHandle { id, path }
    }

    pub async fn load(self, api: &Api) -> Result<Vec<WebLectureVideo>> {
        let weblecture_resp = api
            .api_as_json::<Option<WebLectureResponse>>(
                &format!("weblecture/?ParentID={}", self.id),
                Method::GET,
                None,
            )
            .await?;

        match weblecture_resp {
            Some(weblecture) => {
                let weblectures_resp = api
                    .api_as_json::<ApiData<Vec<WebLectureMedia>>>(
                        &format!("weblecture/{}/sessions", weblecture.id),
                        Method::GET,
                        None,
                    )
                    .await?;

                match weblectures_resp.data {
                    Some(weblectures) => {
                        future::join_all(
                            weblectures
                                .into_iter()
                                .map(|w| Self::load_weblecture(&self, api, w, &self.path)),
                        )
                        .await
                        .into_iter()
                        .collect::<Result<Vec<_>>>()
                    },
                    None => Err("Invalid API response from server: type mismatch"),
                }
            },
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    async fn load_weblecture(&self, api: &Api, weblecture: WebLectureMedia, path: &Path) -> Result<WebLectureVideo> {
        let query_params_resp = api
            .api_as_json::<Option<PanoptoRequestConstructionDetails>>(
                &format!("lti/Launch/panopto?context_id={}&resource_link_id={}&returnURL={}",
                         self.id,
                         weblecture.id,
                        "https://luminus.nus.edu.sg/iframe/lti-return/panopto"),
                Method::GET,
                None,
            )
            .await?;

        match query_params_resp {
            Some(query_params) => {
                let url = Url::parse(&query_params.launchURL).expect("Unable to parse web lecture URL");

                let mut form = HashMap::new();
                query_params.data_items
                    .into_iter()
                    .for_each(|item| { form.insert(item.key.to_string(), item.value.to_string()); });

                let html = api
                    .get_html(&url.to_string(), Method::POST, Some(&form))
                    .await?;

                // TODO: parse HTML, extract video URL
                println!("{}", &html[0..500]);

                Ok(WebLectureVideo {
                    video_url: "".to_string(),
                    path: path.join(Self::make_mp4_extension(Path::new(
                        &sanitise_filename(&weblecture.name),
                    ))),
                    last_updated: parse_time(&weblecture.last_updated_date),
                })
            },
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    // TODO: check file extension
    fn make_mp4_extension(path: &Path) -> PathBuf {
        path.with_extension("mp4")
    }
}

#[async_trait(?Send)]
impl Resource for WebLectureVideo {
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
            // TODO: update.
            move |_| future::ready(Ok(self.video_url.as_str())),
            move |api, stream_url_path, temp_destination| {
                Self::stream_video(api, stream_url_path, temp_destination)
            },
        )
        .await
    }
}

impl WebLectureVideo {
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
