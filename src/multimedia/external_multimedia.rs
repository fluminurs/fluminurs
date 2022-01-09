use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};

use crate::multimedia::Channel;
use crate::panopto;
use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource};
use crate::streamer::stream_and_mux_videos;
use crate::util::sanitise_filename;
use crate::{Api, Result};

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
    #[serde(rename = "DeliveryID")]
    delivery_id: String,
    session_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalMultimediaRequest {
    pub query_parameters: ExternalMultimediaRequestQueryParameters,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalMultimediaRequestQueryParameters {
    #[serde(rename = "folderID")]
    pub folder_id: String,
}

#[derive(Debug, Clone)]
pub struct ExternalVideo {
    id: String,
    path: PathBuf,
}

pub(super) async fn load_external_channel(
    api: &Api,
    channel: Channel,
    path: &Path,
) -> Result<Vec<ExternalVideo>> {
    let channel_path = path.join(Path::new(&sanitise_filename(&channel.name)));

    let response = panopto::launch(
        api,
        &format!("lti/Launch/mediaweb?context_id={}", channel.id),
    )
    .await?;

    // response.url() looks like this: https://mediaweb.ap.panopto.com/Panopto/Pages/Sessions/List.aspx?embedded=1#folderID="xxxxxx"
    // where 'xxxxxx' (without quotes) is the thing we want to extract
    let query_parameters: ExternalMultimediaRequestQueryParameters = response
        .url()
        .fragment()
        .ok_or("Query parameters missing from external multimedia response")
        .and_then(|s| {
            serde_urlencoded::from_str(s).map_err(|_| {
                "Failed to decode external multimedia request query parameters to get folder ID"
            })
        })
        .and_then(|qp: ExternalMultimediaRequestQueryParameters| {
            // we have to remove the quotes manually because Panopto uses some kind of non-standard encoding
            let s = qp.folder_id.as_str();
            let err = Err("Cannot parse external multimedia folder ID");
            if s.len() <= 2 {
                return err;
            }
            let (tmp, last) = s.split_at(s.len() - 1);
            let (first, mid) = tmp.split_at(1);
            if first != "\"" || last != "\"" {
                err
            } else {
                Ok(ExternalMultimediaRequestQueryParameters {
                    folder_id: mid.to_string(),
                })
            }
        })?;

    let panopto_url =
        Url::parse("https://mediaweb.ap.panopto.com/Panopto/Services/Data.svc/GetSessions")
            .expect("Invalid URL");

    let json = ExternalMultimediaRequest { query_parameters };

    let response = api
        .custom_request(panopto_url, Method::POST, None, |req| req.json(&json))
        .await?;

    let output = response
        .json::<ExternalMultimediaResponse>()
        .await
        .map_err(|_| "Unable to deserialize JSON")?;

    Ok(output
        .d
        .results
        .into_iter()
        .map(|m| ExternalVideo {
            id: m.delivery_id,
            path: channel_path.join(super::make_mp4_extension(Path::new(&sanitise_filename(
                &m.session_name,
            )))),
        })
        .collect::<Vec<_>>())
}

#[async_trait]
impl Resource for ExternalVideo {
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
        // External multimedia do not have last updated dates
        SystemTime::UNIX_EPOCH
    }

    async fn download(
        &self,
        api: &Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult> {
        let delivery_id: &str = self.id();
        resource::do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.last_updated(),
            move |api| panopto::get_stream_specs(api, delivery_id),
            move |api, stream_specs, temp_destination| async move {
                stream_and_mux_videos(api, &stream_specs, temp_destination).await
            },
        )
        .await
    }
}
