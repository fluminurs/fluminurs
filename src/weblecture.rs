use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use reqwest::Method;
use serde::Deserialize;

use crate::panopto;
use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource};
use crate::streamer::{stream_and_mux_videos, StreamSpec};
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebLectureResponse {
    id: String,
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

#[derive(Debug, Clone)]
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

#[async_trait]
impl Resource for WebLectureVideo {
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
        let context_id: &str = &self.module_id;
        let resource_link_id: &str = &self.id;
        resource::do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.last_updated(),
            move |api| launch_panopto_and_get_stream_specs(api, context_id, resource_link_id),
            move |api, stream_specs, temp_destination| async move {
                stream_and_mux_videos(api, &stream_specs, temp_destination).await
            },
        )
        .await
    }
}

async fn launch_panopto_and_get_stream_specs(
    api: &Api,
    context_id: &str,
    resource_link_id: &str,
) -> Result<Vec<StreamSpec>> {
    let response = panopto::launch(
        api,
        &format!(
            "lti/Launch/panopto?context_id={}&resource_link_id={}",
            context_id, resource_link_id
        ),
    )
    .await?;

    let delivery_id_opt =
        response
            .url()
            .query_pairs()
            .find_map(|(k, v)| if k == "id" { Some(v) } else { None });

    if let Some(delivery_id) = delivery_id_opt {
        panopto::get_stream_specs(api, &delivery_id).await
    } else {
        Err("Unable to get \"id\" query parameter of Panopto viewer")
    }
}
