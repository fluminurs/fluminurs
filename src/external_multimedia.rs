use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::{header::USER_AGENT, Method, Url};
use serde::{Deserialize, Serialize};

use crate::multimedia::Channel;
use crate::resource::SimpleDownloadableResource;
use crate::util::sanitise_filename;
use crate::weblecture::WebLectureVideo;
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalMultimediaMedia {
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PanoptoQueryParameter {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalMultimediaResponse {
    d: ExternalMultimediaResponseResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExternalMultimediaResponseResponse {
    results: Vec<ExternalMultimediaIndividualResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExternalMultimediaIndividualResponse {
    viewer_url: String,
    session_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalMultimediaRequest {
    pub query_parameters: ExternalMultimediaRequestQueryParameters,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalMultimediaRequestQueryParameters {
    #[serde(rename = "folderID")]
    pub folder_id: String,
}

pub struct ExternalMultimediaHandle {
    id: String,
    path: PathBuf,
}

pub struct ExternalMultimediaVideo {
    html_url: String,
    path: PathBuf,
    last_updated: SystemTime,
}

impl ExternalMultimediaHandle {
    pub fn new(id: String, path: PathBuf) -> ExternalMultimediaHandle {
        ExternalMultimediaHandle { id, path }
    }

    pub async fn load(self, api: &Api) -> Result<Vec<ExternalMultimediaVideo>> {
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
                    // Only load external multimedia resources
                    // TODO: figure out how to integrate with `multimedia`
                    .filter(|c| c.is_external_tool)
                    .map(|c| Self::load_external_channel(api, c, &self.path)),
            )
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .map(|v| v.into_iter().flatten().collect::<Vec<_>>()),
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    async fn load_external_channel(
        api: &Api,
        channel: Channel,
        path: &Path,
    ) -> Result<Vec<ExternalMultimediaVideo>> {
        let channel_path = path.join(Path::new(&sanitise_filename(&channel.name)));

        let query_params_resp = api
            .api_as_json::<Option<PanoptoRequestConstructionDetails>>(
                &format!("lti/Launch/mediaweb?context_id={}", channel.id),
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

                let response = api
                    .custom_request(url, Method::POST, Some(&form), move |req| {
                        // Panapto displays a 500 internal server error page without a desktop user-agent
                        req.header(
                            USER_AGENT,
                            "Mozilla/5.0 (X11; Linux x86_64; rv:88.0) Gecko/20100101 Firefox/88.0",
                        )
                    })
                    .await?;

                // TODO: NO HARDCODE!>! :P
                let query = response.url().fragment();
                let folder_id = if let Some(fragment) = query {
                    Some(&fragment[12..fragment.len() - 3])
                } else {
                    None
                };

                match folder_id {
                    Some(folder_id) => {
                        let panopto_url = Url::parse(
                            "https://mediaweb.ap.panopto.com/Panopto/Services/Data.svc/GetSessions",
                        )
                        .map_err(|_| "Invalid URL")?;

                        let json = ExternalMultimediaRequest {
                            query_parameters: ExternalMultimediaRequestQueryParameters {
                                folder_id: folder_id.to_string(),
                            },
                        };

                        let response = api
                            .custom_request(panopto_url, Method::POST, None, move |req| {
                                req.json(&json)
                            })
                            .await?;

                        let output = response
                            .json::<ExternalMultimediaResponse>()
                            .await
                            .map_err(|_| "Unable to deserialize JSON")?;

                        Ok(output
                            .d
                            .results
                            .into_iter()
                            .map(|m| ExternalMultimediaVideo {
                                html_url: m.viewer_url,
                                path: channel_path.join(Self::make_mp4_extension(Path::new(
                                    &sanitise_filename(&m.session_name),
                                ))),
                                // TODO: last updated for external multimedia
                                // last_updated: parse_time(&m.last_updated_date),
                                last_updated: SystemTime::UNIX_EPOCH,
                            })
                            .collect::<Vec<_>>())
                    }
                    None => Err("No folder ID"),
                }
            }
            None => Err("Invalid API response from server: type mismatch"),
        }
    }

    // TODO: check file extension?
    fn make_mp4_extension(path: &Path) -> PathBuf {
        path.with_extension("mp4")
    }
}

#[async_trait(?Send)]
impl SimpleDownloadableResource for ExternalMultimediaVideo {
    fn path(&self) -> &Path {
        &self.path
    }

    fn get_last_updated(&self) -> SystemTime {
        self.last_updated
    }

    async fn get_download_url(&self, api: &Api) -> Result<Url> {
        let url =
            Url::parse(&self.html_url).map_err(|_| "Unable to parse external multimedia URL")?;

        let html = api.get_text(url, Method::GET, None).await?;

        // TODO: abstract somewhere else?
        let video_url = WebLectureVideo::extract_video_url_from_document(&html);

        match video_url {
            Some(url) => Ok(Url::parse(&url).map_err(|_| "Unable to parse video URL")?),
            None => Err("Unable to parse HTML"),
        }
    }
}
