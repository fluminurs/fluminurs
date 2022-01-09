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
    #[serde(rename = "isPublishRecordURL")]
    is_publish_record_url: bool, // not sure if we should use this or recordType == 1
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

#[derive(Debug, Clone)]
pub struct ZoomRecording {
    id: String, // note: this is not necessarily unique,
    // but it will only be non-unique if the same conference has multiple recordings,
    // which is okay (but ugly) because append_number() will be called to give each one a unique name.
    // todo: make it less ugly... append the ID before the number is appended?
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
        // It appears that if `is_publish_record_url: false` (todo: ... or is it recordType == 1?)
        // then the recording link will be unclickable on Luminus,
        // so there's probably really no recording then
        // We also poll future meetings, since the meeting time is just a guideline anyway.
        // todo: We could be nicer to the server by looking at our local files first,
        // and only polling those that we don't have, since recordings can't be updated.
        match conferencing_resp.data {
            Some(conferences) => future::join_all(
                conferences
                    .into_iter()
                    .filter(|c| c.is_publish_record_url)
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
    // Sometimes, we also get code: 404, even though the meeting actually exists,
    // but sometimes 404 means that there's really no recording for the meeting ... let's just try 5 times before failing?
    let request_path = format!("zoom/Meeting/{}/cloudrecord", conference.id);
    let mut num_404_tries = 0;
    let cloud_record = loop {
        let cloud_record = api
            .api_as_json::<CloudRecord>(&request_path, Method::GET, None)
            .await?;
        if cloud_record.code != Some(400) && (cloud_record.code != Some(404) || num_404_tries >= 5)
        {
            break cloud_record;
        }
        if cloud_record.code == Some(404) {
            num_404_tries += 1;
        }
    };

    let start_date = parse_time(&conference.start_date);
    let mut conference_id = conference.id;
    let conference_name: &str = &conference.name;

    match cloud_record.record_instances {
        Some(record_instances) => Ok(match record_instances.len() {
            0 => vec![],
            1 => record_instances
                .into_iter()
                .map(|cri| ZoomRecording {
                    id: std::mem::take(&mut conference_id), // ok to use std::mem::take because this lambda is only called once
                    path: path.join(make_mp4_extension(Path::new(&sanitise_filename(
                        conference_name,
                    )))),
                    share_url: cri.share_url,
                    password: cri.password,
                    start_date,
                })
                .collect::<Vec<_>>(),
            _ => record_instances
                .into_iter()
                .enumerate()
                .map(|(i, cri)| ZoomRecording {
                    id: conference_id.clone(),
                    path: path.join(make_mp4_extension(Path::new(&append_number(
                        &sanitise_filename(conference_name),
                        i + 1,
                    )))),
                    share_url: cri.share_url,
                    password: cri.password,
                    start_date,
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

#[async_trait]
impl Resource for ZoomRecording {
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
        self.start_date
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

            let id = {
                let document = Html::parse_document(&html);
                let id_selector = Selector::parse("#meetId").unwrap();

                document
                    .select(&id_selector)
                    .next()
                    .and_then(|el| el.value().attr("value"))
                    .ok_or("Unable to find conference id")?
                    .to_owned()
            };

            let mut form: HashMap<&str, &str> = HashMap::new();
            form.insert("id", &id);
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
