use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use reqwest::header::REFERER;
use reqwest::{Method, Url};
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource};
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

const ZOOM_VALIDATE_MEETING_PASSWORD_URL: &str = "https://nus-sg.zoom.us/rec/validate_meet_passwd";
const ZOOM_PASSWORD_URL_PREFIX: &str = "/rec/share";
const ZOOM_DOWNLOAD_REFERER_URL: &str = "https://nus-sg.zoom.us/";

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
    code: Option<u32>,
    record_instances: Option<Vec<CloudRecordInstance>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudRecordInstance {
    #[serde(rename = "shareURL")]
    share_url: String,
    password: String,
}

// E.g. {"status":true,"errorCode":0,"errorMessage":null,"result":"viewdetailpage"}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZoomValidationResponse {
    status: bool,
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

    // loads all conferences
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
    // Note: Sometimes, we get back {"code":400,"status":"fail","message":"TooManyRequests"}
    // which is probably similar to the comment in infinite_retry_http, but only now it is not a HTTP error code.
    // When this happens, we should retry until succeeded.
    let request_path = format!("zoom/Meeting/{}/cloudrecord", conference.id);
    let cloud_record = loop {
        let cloud_record = api
            .api_as_json::<CloudRecord>(&request_path, Method::GET, None)
            .await?;
        if cloud_record.code != Some(400) {
            break cloud_record;
        }
    };

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
        resource::do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.start_date,
            move |api| self.get_download_url(api),
            move |api, url, temp_destination| {
                resource::download_chunks(api, url, temp_destination, |req| {
                    Api::add_desktop_user_agent(req)
                        .header(reqwest::header::RANGE, "bytes=0-")
                        .header(reqwest::header::REFERER, ZOOM_DOWNLOAD_REFERER_URL)
                })
            },
        )
        .await
    }
}
impl ZoomRecording {
    async fn get_download_url(&self, api: &Api) -> Result<Url> {
        let share_url = Url::parse(&self.share_url).map_err(|_| "Unable to parse share URL")?;
        let share_resp = api
            .custom_request(
                share_url.clone(),
                Method::GET,
                None,
                Api::add_desktop_user_agent,
            )
            .await?;
        let video_resp = if share_resp
            .url()
            .path()
            .starts_with(ZOOM_PASSWORD_URL_PREFIX)
        {
            // we need a password

            let cloned_share_resp_url = share_resp.url().to_string();

            let html = share_resp
                .text()
                .await
                .map_err(|_| "Unable to get HTML response")?;
            let document = Html::parse_document(&html);
            let id_selector = Selector::parse("#meetId").unwrap();

            let mut form: HashMap<&str, &str> = HashMap::new();
            form.insert(
                "id",
                document
                    .select(&id_selector)
                    .next()
                    .and_then(|el| el.value().attr("value"))
                    .ok_or("Unable to find conference id")?,
            );
            form.insert("passwd", &self.password);
            form.insert("action", "viewdetailpage");
            form.insert("recaptcha", "");

            let validate_resp = api
                .custom_request(
                    Url::parse(ZOOM_VALIDATE_MEETING_PASSWORD_URL)
                        .expect("Unable to parse Zoom validation URL"),
                    Method::POST,
                    Some(&form),
                    move |req| {
                        Api::add_desktop_user_agent(req)
                            .header(REFERER, cloned_share_resp_url.as_str())
                    },
                )
                .await?;
            let validate_resp_data = validate_resp
                .json::<ZoomValidationResponse>()
                .await
                .map_err(|_| "Unable to parse response JSON from Zoom validation")?;

            if !validate_resp_data.status {
                return Err("Recording password was rejected by Zoom");
            }

            let resp = api
                .custom_request(share_url, Method::GET, None, Api::add_desktop_user_agent)
                .await?;

            if resp.url().path().starts_with(ZOOM_PASSWORD_URL_PREFIX) {
                // Zoom still wants a password, so we probably failed to get in
                return Err("Zoom still wants a password even though we already supplied it");
            }

            resp
        } else {
            // we don't need a password
            share_resp
        };

        let resp_html = video_resp
            .text()
            .await
            .map_err(|_| "Unable to get response text")?;

        // We use regex here because we're trying to get some data from an embedded javascript script
        let video_url_regex =
            regex::Regex::new("viewMp4Url:[\\s]*\'([^\']*)\'").expect("Unable to parse regex");
        let url = Url::parse(
            video_url_regex
                .captures(&resp_html)
                .ok_or("Parse error")?
                .get(1)
                .ok_or("Parse error")?
                .as_str(),
        )
        .map_err(|_| "Unable to parse conference download URL")?;

        Ok(url)
    }
}
