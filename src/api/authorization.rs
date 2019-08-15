use crate::Result;

use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method, RedirectPolicy, Response};
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

const ADFS_OAUTH2_URL: &str = "https://vafs.nus.edu.sg/adfs/oauth2/authorize";
const ADFS_CLIENT_ID: &str = "E10493A3B1024F14BDC7D0D8B9F649E9-234390";
const ADFS_RESOURCE_TYPE: &str = "sg_edu_nus_oauth";
const ADFS_REDIRECT_URI: &str = "https://luminus.nus.edu.sg/auth/callback";
const API_BASE_URL: &str = "https://luminus.nus.edu.sg/v2/api/";
const OCM_APIM_SUBSCRIPTION_KEY: &str = "6963c200ca9440de8fa1eede730d8f7e";

#[derive(Debug)]
pub struct Authorization {
    pub jwt: Option<String>,
    pub client: Client
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String
}

fn full_api_url(path: &str) -> Url {
    Url::parse(API_BASE_URL)
        .and_then(|u| u.join(path))
        .expect("Unable to join URL's")
}

fn build_auth_url() -> Url {
    let nonce = generate_random_bytes(16);
    let mut url = Url::parse(ADFS_OAUTH2_URL)
            .expect("Unable to parse ADFS URL");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", ADFS_CLIENT_ID)
        .append_pair("state", &nonce)
        .append_pair("redirect_uri", ADFS_REDIRECT_URI)
        .append_pair("scope", "")
        .append_pair("resource", ADFS_RESOURCE_TYPE)
        .append_pair("nonce", &nonce);
    url
}

fn build_auth_form<'a>(username: &'a str, password: &'a str) -> HashMap<&'static str, &'a str> {
    let mut map = HashMap::new();
    map.insert("UserName", username);
    map.insert("Password", password);
    map.insert("AuthMethod", "FormsAuthentication");
    map
}

fn build_token_form(code: &str) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    map.insert("grant_type", "authorization_code");
    map.insert("client_id", ADFS_CLIENT_ID);
    map.insert("resource", ADFS_RESOURCE_TYPE);
    map.insert("code", code);
    map.insert("redirect_uri", ADFS_REDIRECT_URI);
    map
}

fn build_client() -> Result<Client> {
    Client::builder()
        .http1_title_case_headers()
        .cookie_store(true)
        .redirect(RedirectPolicy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.too_many_redirects()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Unable to create HTTP client")
}

pub fn generate_random_bytes(size: usize) -> String {
    (0..size)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

impl Authorization {
    pub fn new() -> Result<Authorization> {
        Ok(Authorization {
            jwt: None,
            client: build_client()?,
        })
    }

    pub fn api(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> Result<Response> {
        let url = full_api_url(path);
        let token = self.jwt.clone().ok_or("Please login first")?;
        let mut request_builder = self
            .client
            .request(method, url)
            .header("Ocp-Apim-Subscription-Key", OCM_APIM_SUBSCRIPTION_KEY)
            .header(CONTENT_TYPE, "application/json")
            .bearer_auth(token);
        if let Some(form) = form {
            request_builder = request_builder.form(form);
        }
        let response = request_builder.send().map_err(|_| "Failed API request")?;
        Ok(response)
    }

    fn auth_http_post(
        &mut self,
        url: Url,
        form: Option<&HashMap<&str, &str>>,
        with_apim: bool
    ) -> Result<Response> {
        let mut request_builder = self.client.request(Method::POST, url);
        if with_apim {
            request_builder = request_builder
                .header("Ocp-Apim-Subscription-Key", OCM_APIM_SUBSCRIPTION_KEY);
        }
        if let Some(form) = form {
            request_builder = request_builder.form(form);
        }
        Ok(request_builder.send().unwrap())
    }

    pub fn login(&mut self, username: &str, password: &str) -> Result<bool> {
        let params = build_auth_form(username, password);
        let auth_resp = self.auth_http_post(build_auth_url(), Some(&params), false)?;
        if !auth_resp.url().as_str().starts_with(ADFS_REDIRECT_URI) {
            return Err("Invalid credentials");
        }
        let (_, code) = auth_resp.url().query_pairs().find(|(key, _)| key == "code")
            .ok_or("Unknown authentication failure (no code returned)")?;

        let params = build_token_form(&code);
<<<<<<< HEAD
        let mut token_resp = self.auth_http_post(full_api_url("login/adfstoken"), Some(&params), true)?;
=======
        let mut token_resp = self.auth_http_post(full_api_url("/login/adfstoken"), Some(&params), true)?;
>>>>>>> b5fbd20... Use ADFS to authenticate
        if !token_resp.status().is_success() {
            return Err("Unknown authentication failure (no token returned)");
        }

        let token_resp_de: TokenResponse = token_resp.json()
            .map_err(|_| "Failed to deserialise token exchange response")?;
        self.jwt = Some(token_resp_de.access_token);
        Ok(true)
    }
}
