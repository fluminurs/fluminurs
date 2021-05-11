use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use reqwest::{Method, Url};
use serde::Deserialize;

use crate::panopto;
use crate::resource::SimpleDownloadableResource;
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

pub struct WebLectureHandle {
    id: String,
    path: PathBuf,
}

pub struct WebLectureVideo {
    module_id: String,
    id: String,
    path: PathBuf,
    last_updated: SystemTime,
}

impl WebLectureHandle {
    pub fn new(id: String, path: PathBuf) -> WebLectureHandle {
        WebLectureHandle { id, path }
    }

    pub async fn load(self, api: &Api) -> Result<Vec<WebLectureVideo>> {
        let weblecture_resp = api
            .api_as_json::<WebLectureResponse>(
                &format!("weblecture/?ParentID={}", self.id),
                Method::GET,
                None,
            )
            .await;

        match weblecture_resp {
            Ok(weblecture) => {
                let weblectures_resp = api
                    .api_as_json::<ApiData<Vec<WebLectureMedia>>>(
                        &format!("weblecture/{}/sessions", weblecture.id),
                        Method::GET,
                        None,
                    )
                    .await?;

                match weblectures_resp.data {
                    Some(weblectures) => Ok(weblectures
                        .into_iter()
                        .map(|w| WebLectureVideo {
                            module_id: self.id.clone(),
                            id: w.id,
                            path: self.path.join(Self::make_mp4_extension(Path::new(
                                &sanitise_filename(&w.name),
                            ))),
                            last_updated: parse_time(&w.last_updated_date),
                        })
                        .collect::<Vec<_>>()),
                    None => Err("Invalid API response from server: type mismatch"),
                }
            }
            // If an error occurred, there are no weblectures for that module
            Err(_) => Ok(vec![]),
        }
    }

    // TODO: check file extension?
    fn make_mp4_extension(path: &Path) -> PathBuf {
        path.with_extension("mp4")
    }
}

#[async_trait(?Send)]
impl SimpleDownloadableResource for WebLectureVideo {
    fn path(&self) -> &Path {
        &self.path
    }

    fn get_last_updated(&self) -> SystemTime {
        self.last_updated
    }

    async fn get_download_url(&self, api: &Api) -> Result<Url> {
        let response = panopto::launch_panopto(
            api,
            &format!(
                "lti/Launch/panopto?context_id={}&resource_link_id={}",
                self.module_id, self.id
            ),
        )
        .await?;

        let html = response
            .text()
            .await
            .map_err(|_| "Unable to get HTML response")?;

        panopto::extract_video_url_from_document(&html)
    }
}
