use std::path::Path;
use std::sync::{Arc, RwLock};

use futures::{future, Future, IntoFuture, Stream};
use futures::future::Either;
use reqwest::Method;
use serde::Deserialize;
use url::Url;

use crate::api::{Api, ApiData, Data};
use crate::Error;

#[derive(Debug, Deserialize)]
struct Access {
    #[serde(rename = "access_Full")]
    full: bool,
    #[serde(rename = "access_Read")]
    read: bool,
    #[serde(rename = "access_Create")]
    create: bool,
    #[serde(rename = "access_Update")]
    update: bool,
    #[serde(rename = "access_Delete")]
    delete: bool,
    #[serde(rename = "access_Settings_Read")]
    settings_read: bool,
    #[serde(rename = "access_Settings_Update")]
    settings_update: bool,
}

#[derive(Debug, Deserialize)]
pub struct Announcement {
    pub title: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct Module {
    pub id: String,
    #[serde(rename = "name")]
    pub code: String,
    #[serde(rename = "courseName")]
    pub name: String,
    access: Access,
    pub term: String,
}

impl Module {
    pub fn is_teaching(&self) -> bool {
        let access = &self.access;
        access.full
            || access.create
            || access.update
            || access.delete
            || access.settings_read
            || access.settings_update
    }

    pub fn is_taking(&self) -> bool {
        !self.is_teaching()
    }

    pub fn get_announcements(&self, api: &Api, archived: bool)
        -> impl Future<Item=Vec<Announcement>, Error=Error> + 'static {
        let path = format!(
            "announcement/{}/{}?sortby=displayFrom%20ASC",
            if archived { "Archived" } else { "NonArchived" },
            self.id
        );
        api.api_as_json::<ApiData>(&path, Method::GET, None)
            .and_then(|api_data| {
                if let Data::Announcements(announcements) = api_data.data {
                    Ok(announcements)
                } else if let Data::Empty(_) = api_data.data {
                    Ok(Vec::new())
                } else {
                    Err("Invalid API response from server: type mismatch")
                }.into_future()
            })
    }

    pub fn as_file(&self) -> File {
        File {
            inner: Arc::new(FileInner {
                id: self.id.to_owned(),
                name: sanitise_filename(self.code.to_owned()),
                is_directory: true,
                children: RwLock::new(None),
                allow_upload: false,
            })
        }
    }
}

struct FileInner {
    id: String,
    name: String,
    is_directory: bool,
    children: RwLock<Option<Vec<File>>>,
    allow_upload: bool,
}

#[derive(Clone)]
pub struct File {
    inner: Arc<FileInner>
}

fn sanitise_filename(name: String) -> String {
    if cfg!(windows) {
        sanitize_filename::sanitize_with_options(
            name.trim(),
            sanitize_filename::Options {
                windows: true,
                truncate: true,
                replacement: "-",
            },
        )
    } else {
        ["\0", "/"].iter().fold(name, |acc, x| acc.replace(x, "-"))
    }
}

impl File {
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn is_directory(&self) -> bool {
        self.inner.is_directory
    }

    pub fn children(&self) -> Option<Vec<File>> {
        self.inner.children.read()
            .expect("Failed to lock mutex")
            .clone()
    }

    pub fn load_children(&self, api: &Api)
        -> impl Future<Item=(), Error=Error> + 'static {
        let apic = api.clone();
        if !self.inner.is_directory {
            return Either::B(Either::A(self.inner.children.write()
                .map(|mut ptr| {
                    *ptr = Some(Vec::new());
                    ()
                })
                .map_err(|_| "Failed to acquire write lock on File")
                .into_future()))
        }
        if self.inner.children.read()
            .map(|children| children.is_some())
            .unwrap_or(false) {
            return Either::A(Ok(()).into_future());
        }
        let subdirs_future = apic.api_as_json::<ApiData>(
            &format!("files/?ParentID={}", self.inner.id),
            Method::GET,
            None
        ).map(|subdirs_data| match subdirs_data.data {
            Data::ApiFileDirectory(subdirs) => subdirs
                .into_iter()
                .map(|s| File {
                    inner: Arc::new(FileInner {
                        id: s.id,
                        name: sanitise_filename(s.name),
                        is_directory: true,
                        children: RwLock::new(None),
                        allow_upload: s.allow_upload.unwrap_or(false),
                    })
                })
                .collect::<Vec<File>>(),
            _ => Vec::<File>::new(),
        });

        let allow_upload = self.inner.allow_upload;
        let files_future = apic.api_as_json::<ApiData>(
            &format!(
                "files/{}/file{}",
                self.inner.id,
                if self.inner.allow_upload {
                    "?populate=Creator"
                } else {
                    ""
                }
            ),
            Method::GET,
            None,
        ).map(move |files_data| match files_data.data {
            Data::ApiFileDirectory(files) => files
                .into_iter()
                .map(|s| File {
                    inner: Arc::new(FileInner {
                        id: s.id,
                        name: sanitise_filename(format!(
                            "{}{}",
                            if allow_upload {
                                format!("{} - ", s.creator_name.unwrap_or("Unknown".to_string()))
                            } else {
                                "".to_string()
                            },
                            s.name
                        )),
                        is_directory: false,
                        children: RwLock::new(Some(Vec::new())),
                        allow_upload: false,
                    })
                })
                .collect::<Vec<File>>(),
            _ => Vec::<File>::new(),
        });

        let self_clone = self.clone();
        Either::B(Either::B(subdirs_future.join(files_future)
            .and_then(move |(mut subdirs, mut files)| {
                subdirs.append(&mut files);
                self_clone.inner.children.write()
                    .map(|mut ptr| {
                        *ptr = Some(subdirs);
                        ()
                    })
                    .map_err(|_| "Failed to acquire write lock on File")
                    .into_future()
            })))
    }

    pub fn load_all_children(&self, api: &Api)
        -> impl Future<Item=(), Error=Error> + 'static {
        fn load_all_children_helper((check, api, state): (bool, Api, Vec<File>))
            -> impl Future<Item=future::Loop<(), (bool, Api, Vec<File>)>, Error=Error> {
            if check {
                // Collect the children of the files and load them
                let new_state = state.into_iter()
                    .flat_map(|file| file.children()
                        .expect("Children must have been loaded")
                        .into_iter())
                    .collect::<Vec<File>>();
                Either::A(if new_state.is_empty() {
                    future::result(Ok(future::Loop::Break(())))
                } else {
                    future::result(Ok(future::Loop::Continue((!check, api, new_state))))
                })
            } else {
                let apic = api.clone();
                Either::B(future::join_all(state.into_iter()
                    .map(move |file| file.load_children(&apic)
                        .map(|_| file)))
                    .map(|files| future::Loop::Continue((true, api, files))))
            }
        }

        let selfc = self.clone();
        let apic = api.clone();
        self.load_children(api)
            .and_then(move |_| future::loop_fn((false, apic, vec![selfc]),
                load_all_children_helper)
            .map(|_| ()))
    }

    pub fn get_download_url(&self, api: Api)
        -> impl Future<Item=Url, Error=Error> + 'static {
        api.api_as_json::<ApiData>(
            &format!("files/file/{}/downloadurl", self.inner.id),
            Method::GET,
            None,
        ).and_then(|api_data|
            if let Data::Text(url) = api_data.data {
                Ok(Url::parse(&url).map_err(|_| "Unable to parse URL")?)
            } else {
                Err("Invalid API response from server: type mismatch")
            }
        )
    }

    pub fn download(&self, api: Api, path: &Path)
        -> impl Future<Item=bool, Error=Error> + 'static {
        let destination = path.to_path_buf();
        if destination.exists() {
            Either::A(Ok(false).into_future())
        } else {
            let download_future = self.get_download_url(api.clone());
            let create_dir_future = match destination.parent() {
                Some(parent) => Either::A(tokio::fs::create_dir_all(parent.to_path_buf())
                    .map(|_| ())
                    .map_err(|_| "Unable to create directory")),
                None => Either::B(future::result(Ok(())))
            };
            Either::B(create_dir_future.and_then(move |()| tokio::fs::File::create(destination)
                .map_err(|_| "Unable to open file"))
                .and_then(move |file| download_future
                    .and_then(move |download_url|
                        api.get_client()
                            .get(download_url)
                            .send()
                            .map_err(|_| "Failed during download")
                            .and_then(|r| r.into_body()
                                .map_err(|_| "Failed to get file body")
                                .fold(file, |file, chunk| {
                                    tokio::io::write_all(file, chunk)
                                        .map(|(f, _)| f)
                                        .map_err(|_| "Failed writing to disk")
                                }))
                            .map(|_| true)
                    )))
        }
    }
}
