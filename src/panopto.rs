// Utilities for Panopto (web lectures and external multimedia)

use std::collections::HashMap;

use reqwest::{Method, Response, Url};
use serde::Deserialize;

use crate::streamer::StreamSpec;
use crate::{Api, Result};

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

pub async fn launch(api: &Api, api_path: &str) -> Result<Response> {
    let query_params = api
        .api_as_json::<Option<PanoptoRequestConstructionDetails>>(api_path, Method::GET, None)
        .await?
        .ok_or("Invalid API response from server: type mismatch")?;

    let url =
        Url::parse(&query_params.launch_url).map_err(|_| "Unable to parse Panopto launch URL")?;

    let form: HashMap<&str, &str> = query_params
        .data_items
        .iter()
        .map(|item| (item.key.as_str(), item.value.as_str()))
        .collect();

    api.custom_request(url, Method::POST, Some(&form), Api::add_desktop_user_agent)
        .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DeliveryInfo {
    delivery: Delivery,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Delivery {
    streams: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Stream {
    relative_start: f64,
    stream_url: String,
}

pub async fn get_stream_specs(api: &Api, delivery_id: &str) -> Result<Vec<StreamSpec>> {
    let post_data = make_delivery_info_post_data(delivery_id);
    let delivery_info = api
        .custom_request(
            Url::parse("https://mediaweb.ap.panopto.com/Panopto/Pages/Viewer/DeliveryInfo.aspx")
                .expect("Unable to parse Panopto DeliverInfo URL"),
            Method::POST,
            Some(&post_data),
            Api::add_desktop_user_agent,
        )
        .await?
        .json::<DeliveryInfo>()
        .await
        .map_err(|_| "Unable to deserialize JSON")?;

    let streams = delivery_info.delivery.streams;

    if streams.is_empty() {
        Err("No streams available on DeliveryInfo")
    } else {
        Ok(streams
            .into_iter()
            .map(|s| StreamSpec {
                stream_url_path: s.stream_url,
                offset_seconds: s.relative_start,
            })
            .collect())
    }
}

/// Constructs the form params required for the post request to DeliveryInfo.aspx
fn make_delivery_info_post_data(delivery_id: &str) -> HashMap<&str, &str> {
    // These params are used by Panopto's web frontend,
    // so we'll mimic all of it.  Not sure if there are any videos
    // that need different selections of these params.
    let mut post_data: HashMap<&str, &str> = HashMap::new();
    post_data.insert("deliveryId", delivery_id);
    post_data.insert("invocationId", "");
    post_data.insert("isLiveNotes", "false");
    post_data.insert("refreshAuthCookie", "true");
    post_data.insert("isActiveBroadcast", "false");
    post_data.insert("isEditing", "false");
    post_data.insert("isKollectiveAgentInstalled", "false");
    post_data.insert("isEmbed", "false");
    post_data.insert("responseType", "json");
    post_data
}
