// Utilities for Panopto (web lectures and external multimedia)

use std::collections::HashMap;

use reqwest::{Method, Response, Url};
use scraper::{Html, Selector};
use serde::Deserialize;

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

pub async fn launch_panopto(api: &Api, api_path: &str) -> Result<Response> {
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

pub fn extract_video_url_from_document(html: &str) -> Result<Url> {
    let document = Html::parse_document(html);
    let selector = Selector::parse(r#"meta[property="og:video"]"#).unwrap();

    let url_str = document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr("content"))
        .ok_or("Unable to find video URL")?;

    Url::parse(url_str).map_err(|_| "Unable to parse video URL")
}
