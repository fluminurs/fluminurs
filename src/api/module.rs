use std::path::Path;
use std::sync::{Arc, RwLock};

use futures_util::future;
use reqwest::{Method, Url};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::api::{Api, ApiData, Data};
use crate::Result;

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

    pub async fn get_announcements(&self, api: &Api, archived: bool) -> Result<Vec<Announcement>> {
        let path = format!(
            "announcement/{}/{}?sortby=displayFrom%20ASC",
            if archived { "Archived" } else { "NonArchived" },
            self.id
        );
        let api_data = api.api_as_json::<ApiData>(&path, Method::GET, None).await?;
        if let Data::Announcements(announcements) = api_data.data {
            Ok(announcements)
        } else if let Data::Empty(_) = api_data.data {
            Ok(vec![])
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }

    pub fn as_file(&self) -> File {
        File {
            inner: Arc::new(FileInner {
                id: self.id.to_owned(),
                name: sanitise_filename(self.code.to_owned()),
                is_directory: true,
                children: RwLock::new(None),
                allow_upload: false,
            }),
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
    inner: Arc<FileInner>,
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
        self.inner
            .children
            .read()
            .expect("Failed to lock mutex")
            .clone()
    }

    pub async fn load_children(&self, api: &Api) -> Result<()> {
        let apic = api.clone();
        if !self.inner.is_directory {
            return self
                .inner
                .children
                .write()
                .map(|mut ptr| {
                    *ptr = Some(Vec::new());
                    ()
                })
                .map_err(|_| "Failed to acquire write lock on File");
        }
        if self
            .inner
            .children
            .read()
            .map(|children| children.is_some())
            .unwrap_or(false)
        {
            return Ok(());
        }
        let subdirs = apic
            .api_as_json::<ApiData>(
                &format!("files/?ParentID={}", self.inner.id),
                Method::GET,
                None,
            )
            .await?;
        let mut subdirs = match subdirs.data {
            Data::ApiFileDirectory(subdirs) => subdirs
                .into_iter()
                .map(|s| File {
                    inner: Arc::new(FileInner {
                        id: s.id,
                        name: sanitise_filename(s.name),
                        is_directory: true,
                        children: RwLock::new(None),
                        allow_upload: s.allow_upload.unwrap_or(false),
                    }),
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        };

        let allow_upload = self.inner.allow_upload;
        let files = apic
            .api_as_json::<ApiData>(
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
            )
            .await?;
        let mut files = match files.data {
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
                    }),
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        };

        let self_clone = self.clone();
        subdirs.append(&mut files);
        self_clone
            .inner
            .children
            .write()
            .map(|mut ptr| {
                *ptr = Some(subdirs);
            })
            .map_err(|_| "Failed to acquire write lock on File")
    }

    pub async fn load_all_children(&self, api: &Api) -> Result<()> {
        let apic = api.clone();
        self.load_children(api).await?;

        let mut files = vec![self.clone()];
        loop {
            for res in future::join_all(files.iter().map(|file| file.load_children(&apic))).await {
                res?;
            }
            files = files
                .into_iter()
                .flat_map(|file| {
                    file.children()
                        .expect("Children must have been loaded")
                        .into_iter()
                })
                .collect();
            if files.is_empty() {
                break;
            }
        }
        Ok(())
    }

    pub async fn get_download_url(&self, api: Api) -> Result<Url> {
        let data = api
            .api_as_json::<ApiData>(
                &format!("files/file/{}/downloadurl", self.inner.id),
                Method::GET,
                None,
            )
            .await?;
        if let Data::Text(url) = data.data {
            Ok(Url::parse(&url).map_err(|_| "Unable to parse URL")?)
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }

    pub async fn download(&self, api: Api, path: &Path) -> Result<bool> {
        let destination = path.to_path_buf();
        if destination.exists() {
            Ok(false)
        } else {
            let download_url = self.get_download_url(api.clone()).await?;
            match destination.parent() {
                Some(parent) => {
                    tokio::fs::create_dir_all(parent.to_path_buf())
                        .await
                        .map_err(|_| "Unable to create directory")?;
                }
                None => (),
            };
            let mut file = tokio::fs::File::create(destination)
                .await
                .map_err(|_| "Unable to open file")?;
            let mut res = api
                .get_client()
                .get(download_url)
                .send()
                .await
                .map_err(|_| "Failed during download")?;
            while let Some(ref chunk) = res.chunk().await.map_err(|_| "Failed during streaming")? {
                file.write_all(chunk)
                    .await
                    .map_err(|_| "Failed writing to disk")?;
            }
            Ok(true)
        }
    }
}
