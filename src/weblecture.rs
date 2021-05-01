use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::{Method, Url};
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::{file::File, resource};
use crate::resource::{OverwriteMode, OverwriteResult, Resource};
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
    video_url: Url,
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

                let video_url = Self::extract_video_url_from_document(&html);

                match video_url {
                    Some(url) => Ok(WebLectureVideo {
                        video_url: Url::parse(&url).expect("Unable to parse video URL"),
                        path: path.join(Self::make_mp4_extension(Path::new(
                            &sanitise_filename(&weblecture.name),
                        ))),
                        last_updated: parse_time(&weblecture.last_updated_date),
                    }),
                    None => Err("Unable to parse HTML"),
                }
            },
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    fn extract_video_url_from_document(html: &str) -> Option<String> {
        let document = Html::parse_document(html);
        let selector = Selector::parse(r#"meta[property="og:video"]"#).unwrap();

        match document.select(&selector).next() {
            Some(element) => element.value().attr("content").map(|x| x.to_string()),
            None => None,
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
            // TODO: update.
            move |_| future::ready(Ok(self.video_url.clone())),
            move |api, video_url, temp_destination| {
                File::download_chunks(api, video_url, temp_destination)
            },
        )
        .await
    }
}
