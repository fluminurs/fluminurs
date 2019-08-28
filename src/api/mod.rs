use std::collections::HashMap;
use std::sync::Arc;

use futures::{Future, IntoFuture};
use futures::future::{self, Either};
use reqwest::{Method, RedirectPolicy};
use reqwest::header::CONTENT_TYPE;
use reqwest::r#async::{Client, Request, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use url::Url;

use crate::{Error, Result};

use self::module::{Announcement, Module};

pub mod module;

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

fn generate_random_bytes(size: usize) -> String {
    (0..size)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

fn infinite_retry_http<F>(
    client: Client,
    url: Url,
    method: Method,
    form: Option<&HashMap<&str, &str>>,
    edit_request: F
) -> impl Future<Item=Response, Error=Error> + 'static
    where F: (Fn(RequestBuilder) -> RequestBuilder) + 'static {
    fn retry_forever<F>(c: Client, r: F)
        -> impl Future<Item=Response, Error=Error> + 'static
        where F: (Fn(Client) -> Result<Request>) + 'static {
        fn helper<F>((c, r): (Client, F))
            -> impl Future<Item=future::Loop<Response, (Client, F)>, Error=Error>
            where F: (Fn(Client) -> Result<Request>) + 'static {
            match r(c.clone()) {
                Ok(req) => Either::A(c.execute(req)
                    .then(move |result| future::result(Ok(match result {
                        Ok(response) => future::Loop::Break(response),
                        Err(_) => future::Loop::Continue((c, r))
                    })))),
                Err(err) => Either::B(future::result(Err(err)))
            }
        }

        future::loop_fn((c, r), helper)
    }

    let form = match form {
        Some(form) => match serde_urlencoded::to_string(form) {
            Ok(body) => Some(body),
            Err(_) => return Either::A(future::result(Err("Failed to serialise HTTP form")))
        },
        None => None
    };

    let request_builder = move |c: Client| {
        let mut request_builder = c.request(method.clone(), url.clone());
        if let Some(ref form) = form {
            request_builder = request_builder
                .body(form.clone())
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        } else {
            request_builder = request_builder
                .header(CONTENT_TYPE, "application/json");
        }

        edit_request(request_builder).build()
            .map_err(|_| "Failed to build request")
    };

    Either::B(retry_forever(client, request_builder))
}

fn auth_http_post(
    client: Client,
    url: Url,
    form: Option<&HashMap<&str, &str>>,
    with_apim: bool
) -> impl Future<Item=Response, Error=Error> + 'static {
    infinite_retry_http(client, url, Method::POST, form,
        move |req| if with_apim {
            req.header(OCP_APIM_SUBSCRIPTION_KEY_HEADER, OCP_APIM_SUBSCRIPTION_KEY)
        } else {
            req
        })
}

#[derive(Debug, Clone)]
pub struct Api {
    pub jwt: Arc<String>,
    pub client: Client
}

impl Api {
    pub fn get_client(&self) -> &Client {
        &self.client
    }

    fn api_as_json<T: DeserializeOwned + 'static>(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> impl Future<Item=T, Error=Error> + 'static {
        self
            .api(path, method, form)
            .and_then(|mut resp| resp.json::<T>()
                .map_err(|_| "Unable to deserialize JSON"))
    }

    pub fn api(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> impl Future<Item=Response, Error=Error> + 'static {
        let url = full_api_url(path);
        let jwt= self.jwt.clone();

        infinite_retry_http(self.client.clone(), url, method, form,
            move |req| req.header(OCP_APIM_SUBSCRIPTION_KEY_HEADER, OCP_APIM_SUBSCRIPTION_KEY)
                .bearer_auth(jwt.clone()))
    }

    fn current_term(&self) -> impl Future<Item=String, Error=Error> + 'static {
        self.api_as_json::<Term>(
            "setting/AcademicWeek/current?populate=termDetail",
            Method::GET,
            None,
        )
            .map(|term| term.term_detail.term)
    }

    pub fn modules(&self, current_term_only: bool)
        -> impl Future<Item=Vec<Module>, Error=Error> + 'static {
        let current_term = if current_term_only {
            Either::A(self.current_term().map(|s| Some(s)))
        } else {
            Either::B(future::done(Ok(None)))
        };

        let modules = self.api_as_json::<ApiData>("module", Method::GET, None);

        modules.join(current_term)
            .and_then(move |(api_data, current_term)| {
                if let Data::Modules(modules) = api_data.data {
                    future::result(Ok(if let Some(current_term) = current_term {
                        modules
                            .into_iter()
                            .filter(|m| m.term == current_term)
                            .collect()
                    } else {
                        modules
                    }))
                } else if let Data::Empty(_) = api_data.data {
                    future::result(Ok(Vec::new()))
                } else {
                    future::result(Err("Invalid API response from server: type mismatch"))
                }
            })
    }

    pub fn name(&self) -> impl Future<Item=String, Error=Error> + 'static {
        self.api_as_json::<Name>("user/Profile", Method::GET, None)
            .map(|name| name.user_name_original)
    }

    pub fn with_login<'a>(username: &str, password: &str)
        -> impl Future<Item=Api, Error=Error> + 'static {
        let params = build_auth_form(username, password);
        let client = match build_client() {
            Ok(client) => client,
            Err(str) => return Either::A(future::result(Err(str)))
        };
        Either::B(auth_http_post(client.clone(), build_auth_url(), Some(&params), false)
            .map(move |r| (client, r))
            .and_then(|(client, auth_resp)| {
                if !auth_resp.url().as_str().starts_with(ADFS_REDIRECT_URI) {
                    return Either::A(Err("Invalid credentials").into_future());
                }
                let code = auth_resp.url().query_pairs().find(|(key, _)| key == "code")
                    .map(|(_key, code)| code.into_owned());
                let client2 = client.clone();
                Either::B(code
                    .ok_or("Unknown authentication failure (no code returned)")
                    .into_future()
                    .and_then(|code|
                        auth_http_post(client2, full_api_url("login/adfstoken"), Some(&build_token_form(&code)), true))
                    .map(|resp| (client, resp)))
            })
            .and_then(|(client, mut token_resp)| {
                if !token_resp.status().is_success() {
                    return Either::A(Err("Unknown authentication failure (no token returned)").into_future());
                }
                Either::B(token_resp.json::<TokenResponse>()
                    .map_err(|_| "Failed to deserialise token exchange response")
                    .map(|resp| (client, resp)))
            })
            .map(|(client, token_resp_de)| Api {
                jwt: Arc::new(token_resp_de.access_token),
                client
            }))
    }
}
