mod authorization;
pub mod module;

use crate::api::authorization::Authorization;
use crate::api::module::{Announcement, Module};
use crate::Error;
use reqwest::{r#async::Client, Method};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashMap;
use futures::{Future, IntoFuture};
use futures::future::Either;

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

#[derive(Debug, Clone)]
pub struct Api {
    authorization: Authorization,
}

impl Api {
    pub fn with_login<'a>(username: &'a str, password: &'a str) -> impl Future<Item=Api, Error=Error> + 'a {
        Authorization::with_login(username, password)
            .map(|auth| Api {
                authorization: auth
            })
    }

    fn api_as_json<'a, T: DeserializeOwned + 'a>(
        &self,
        path: &str,
        method: Method,
        form: Option<&HashMap<&str, &str>>,
    ) -> impl Future<Item=T, Error=Error> + 'a {
        self
            .authorization
            .api(path, method, form)
            .and_then(|mut resp| resp.json::<T>()
                .map_err(|_| "Unable to deserialize JSON"))
    }

    pub fn name(&self) -> impl Future<Item=String, Error=Error> + '_ {
        self.api_as_json::<Name>("user/Profile", Method::GET, None)
            .map(|name| name.user_name_original)
    }

    fn current_term(&self) -> impl Future<Item=String, Error=Error> + '_ {
        self.api_as_json::<Term>(
                "setting/AcademicWeek/current?populate=termDetail",
                Method::GET,
                None,
            )
            .map(|term| term.term_detail.term)
    }

    pub fn modules(&self, current_term_only: bool)
        -> impl Future<Item=Vec<Module>, Error=Error> + '_ {
        self.api_as_json::<ApiData>("module", Method::GET, None)
            .and_then(move |api_data| {
                if let Data::Modules(modules) = api_data.data {
                    if current_term_only {
                        Either::A(self.current_term()
                            .map(|current_term| modules
                                .into_iter()
                                .filter(|m| m.term == current_term)
                                .collect()))
                    } else {
                        Either::B(Ok(modules).into_future())
                    }
                } else if let Data::Empty(_) = api_data.data {
                    Either::B(Ok(Vec::new()).into_future())
                } else {
                    Either::B(Err("Invalid API response from server: type mismatch").into_future())
                }
            })
    }

    pub fn get_client(&self) -> &Client {
        &self.authorization.client
    }
}
