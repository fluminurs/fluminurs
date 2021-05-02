use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use reqwest::{Method, Url};
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::resource::{OverwriteMode, OverwriteResult, Resource};
use crate::util::{parse_time, sanitise_filename};
use crate::{file::File, resource};
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
    #[serde(rename = "launchURL")]
    launch_url: String,
    data_items: Vec<PanoptoQueryParameter>,
}

#[derive(Debug, Deserialize)]
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
            move |api| self.get_download_url(api),
            move |api, video_url, temp_destination| {
                File::download_chunks(api, video_url, temp_destination)
            },
        )
        .await
    }
}

impl WebLectureVideo {
    async fn get_download_url(&self, api: &Api) -> Result<Url> {
        let query_params_resp = api
            .api_as_json::<Option<PanoptoRequestConstructionDetails>>(
                &format!(
                    "lti/Launch/panopto?context_id={}&resource_link_id={}",
                    self.module_id, self.id
                ),
                Method::GET,
                None,
            )
            .await?;

        match query_params_resp {
            Some(query_params) => {
                let url = Url::parse(&query_params.launch_url)
                    .map_err(|_| "Unable to parse web lecture URL")?;

                let form: HashMap<&str, &str> = query_params
                    .data_items
                    .iter()
                    .map(|item| (item.key.as_str(), item.value.as_str()))
                    .collect();

                let html = api.get_text(url, Method::POST, Some(&form)).await?;

                let video_url = Self::extract_video_url_from_document(&html);

                match video_url {
                    Some(url) => Ok(Url::parse(&url).map_err(|_| "Unable to parse URL")?),
                    None => Err("Unable to parse HTML"),
                }
            }
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    fn extract_video_url_from_document(html: &str) -> Option<String> {
        let document = Html::parse_document(html);
        let selector = Selector::parse(r#"meta[property="og:video"]"#).unwrap();

        document
            .select(&selector)
            .next()
            .and_then(|element| element.value().attr("content").map(|x| x.to_string()))
    }
}
