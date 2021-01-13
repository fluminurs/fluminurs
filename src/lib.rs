use std::collections::HashMap;
use std::sync::Arc;

use reqwest::redirect::Policy;
use reqwest::Method;
use reqwest::{header::CONTENT_TYPE, Certificate};
use reqwest::{Client, RequestBuilder, Response, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use self::module::{Announcement, Module};

pub mod module;

pub type Error = &'static str;
pub type Result<T> = std::result::Result<T, Error>;

const ADFS_OAUTH2_URL: &str = "https://vafs.nus.edu.sg/adfs/oauth2/authorize";
const ADFS_CLIENT_ID: &str = "E10493A3B1024F14BDC7D0D8B9F649E9-234390";
const ADFS_RESOURCE_TYPE: &str = "sg_edu_nus_oauth";
const ADFS_REDIRECT_URI: &str = "https://luminus.nus.edu.sg/auth/callback";
const API_BASE_URL: &str = "https://luminus.nus.edu.sg/v2/api/";
const OCP_APIM_SUBSCRIPTION_KEY: &str = "6963c200ca9440de8fa1eede730d8f7e";
const OCP_APIM_SUBSCRIPTION_KEY_HEADER: &str = "Ocp-Apim-Subscription-Key";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Name {
    user_name_original: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Term {
    term_detail: TermDetail,
}

#[derive(Deserialize)]
struct TermDetail {
    term: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFileDirectory {
    id: String,
    name: String,
    allow_upload: Option<bool>,
    creator_name: Option<String>,
    last_updated_date: String,
}

#[derive(Debug, Deserialize)]
struct ApiData {
    data: Data,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Data {
    Empty(Vec<[(); 0]>),
    Modules(Vec<Module>),
    Announcements(Vec<Announcement>),
    ApiFileDirectory(Vec<ApiFileDirectory>),
    Text(String),
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

fn full_api_url(path: &str) -> Url {
    Url::parse(API_BASE_URL)
        .and_then(|u| u.join(path))
        .expect("Unable to join URL's")
}

fn build_auth_url() -> Url {
    let nonce = generate_random_bytes(16);
    let mut url = Url::parse(ADFS_OAUTH2_URL).expect("Unable to parse ADFS URL");
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

fn build_token_form<'a>(code: &'a str) -> HashMap<&'static str, &'a str> {
    let mut map = HashMap::new();
    map.insert("grant_type", "authorization_code");
    map.insert("client_id", ADFS_CLIENT_ID);
    map.insert("resource", ADFS_RESOURCE_TYPE);
    map.insert("code", code);
    map.insert("redirect_uri", ADFS_REDIRECT_URI);
    map
}

fn hack_get_intermediate_cert() -> Result<Certificate> {
    Certificate::from_pem(include_bytes!("DigiCert_TLS_RSA_SHA256_2020_CA1.pem"))
        .map_err(|_| "Unable to load TLS intermediate certificate")
}

fn build_client() -> Result<Client> {
    Client::builder()
        .http1_title_case_headers()
        .cookie_store(true)
        .add_root_certificate(hack_get_intermediate_cert()?)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Unable to create HTTP client")
}

fn generate_random_bytes(size: usize) -> String {
    (0..size)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

async fn infinite_retry_http<F>(
    client: Client,
    url: Url,
    method: Method,
    form: Option<&HashMap<&str, &str>>,
    edit_request: F,
) -> Result<Response>
where
    F: (Fn(RequestBuilder) -> RequestBuilder),
{
    let form = if let Some(form) = form {
        Some(serde_urlencoded::to_string(form).map_err(|_| "Failed to serialise HTTP form")?)
    } else {
        None
    };

    // LumiNUS randomly returns 400 to a perfectly good request for no apparent reason
    // We'll just ignore it and repeat the request
    let res = loop {
        let mut request_builder = client.request(method.clone(), url.clone());
        if let Some(ref form) = form {
            request_builder = request_builder
                .body(form.clone())
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        } else {
            request_builder = request_builder.header(CONTENT_TYPE, "application/json");
        }
        let request = edit_request(request_builder)
            .build()
            .map_err(|_| "Failed to build request")?;

        let res = client.execute(request).await.map_err(|_| "HTTP error");
        if let Ok(res) = res {
            break res;
        }
    };
    Ok(res)
}

async fn auth_http_post(
    client: Client,
    url: Url,
    form: Option<&HashMap<&str, &str>>,
    with_apim: bool,
) -> Result<Response> {
    infinite_retry_http(client, url, Method::POST, form, move |req| {
        if with_apim {
            req.header(OCP_APIM_SUBSCRIPTION_KEY_HEADER, OCP_APIM_SUBSCRIPTION_KEY)
        } else {
            req
        }
    })
    .await
}

#[derive(Debug, Clone)]
pub struct Api {
    pub jwt: Arc<String>,
    pub client: Client,
}

impl Api {
    pub fn get_client(&self) -> &Client {
        &self.client
    }

    async fn api_as_json<T: DeserializeOwned + 'static>(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> Result<T> {
        let res = self.api(path, method, form).await?;
        res.json::<T>()
            .await
            .map_err(|_| "Unable to deserialize JSON")
    }

    pub async fn api(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> Result<Response> {
        let url = full_api_url(path);
        let jwt = Arc::clone(&self.jwt);

        infinite_retry_http(self.client.clone(), url, method, form, move |req| {
            req.header(OCP_APIM_SUBSCRIPTION_KEY_HEADER, OCP_APIM_SUBSCRIPTION_KEY)
                .bearer_auth(jwt.clone())
        })
        .await
    }

    async fn current_term(&self) -> Result<String> {
        Ok(self
            .api_as_json::<Term>(
                "setting/AcademicWeek/current?populate=termDetail",
                Method::GET,
                None,
            )
            .await?
            .term_detail
            .term)
    }

    pub async fn modules(&self, term: Option<String>) -> Result<Vec<Module>> {
        enum FilterMode {
            GreaterThan(String),
            Equal(String),
        }
        let filter = if let Some(specified_term) = term {
            FilterMode::Equal(specified_term)
        } else {
            /* we want all modules for terms later than or equal to the current term,
            because getting modules for future terms is useful when we are currently in a vacation week */
            FilterMode::GreaterThan(self.current_term().await?)
        };

        let modules = self
            .api_as_json::<ApiData>("module", Method::GET, None)
            .await?;

        if let Data::Modules(modules) = modules.data {
            let iter = modules.into_iter();
            let mut selected_modules: Vec<Module> = match filter {
                FilterMode::Equal(term) => iter.filter(|m| m.term == term).collect(),
                FilterMode::GreaterThan(term) => iter.filter(|m| m.term >= term).collect(),
            };
            // sort by increasing module code, then by decreasing term
            selected_modules.sort_unstable_by(|m1, m2| {
                m1.code.cmp(&m2.code).then_with(|| m2.term.cmp(&m1.term))
            });
            selected_modules.dedup_by(|other, latest| if other.code == latest.code {
                println!("Warning: module {} appeared in more than one semester, only latest semester will be retrieved", other.code);
                true
            } else {
                false
            });
            Ok(selected_modules)
        } else if let Data::Empty(_) = modules.data {
            Ok(vec![])
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }

    pub async fn name(&self) -> Result<String> {
        Ok(self
            .api_as_json::<Name>("user/Profile", Method::GET, None)
            .await?
            .user_name_original)
    }

    pub async fn with_login<'a>(username: &str, password: &str) -> Result<Api> {
        let params = build_auth_form(username, password);
        let client = build_client()?;

        let auth_resp =
            auth_http_post(client.clone(), build_auth_url(), Some(&params), false).await?;
        if !auth_resp.url().as_str().starts_with(ADFS_REDIRECT_URI) {
            return Err("Invalid credentials");
        }
        let code = auth_resp
            .url()
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_key, code)| code.into_owned())
            .ok_or("Unknown authentication failure (no code returned)")?;
        let client2 = client.clone();
        let token_resp = auth_http_post(
            client2,
            full_api_url("login/adfstoken"),
            Some(&build_token_form(&code)),
            true,
        )
        .await?;
        if !token_resp.status().is_success() {
            return Err("Unknown authentication failure (no token returned)");
        }
        let token = token_resp
            .json::<TokenResponse>()
            .await
            .map_err(|_| "Failed to deserialise token exchange response")?;
        Ok(Api {
            jwt: Arc::new(token.access_token),
            client,
        })
    }
}
