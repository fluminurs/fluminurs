use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use futures_util::future::{BoxFuture, FutureExt};
use reqwest::{Method, Url};
use serde::Deserialize;

use crate::resource::SimpleDownloadableResource;
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFileDirectory {
    id: String,
    name: String,
    file_name: Option<String>,
    allow_upload: Option<bool>,
    creator_name: Option<String>,
    last_updated_date: String,
}

pub struct DirectoryHandle {
    id: String,
    path: PathBuf,
    allow_upload: bool,
    /* last_updated: SystemTime, */
}

pub struct File {
    id: String,
    path: PathBuf,
    last_updated: SystemTime,
}

impl DirectoryHandle {
    pub fn new(id: String, path: PathBuf) -> DirectoryHandle {
        DirectoryHandle {
            id,
            path,
            allow_upload: false,
        }
    }

    // loads all files recursively and returns a flattened list
    pub fn load<'a>(
        self,
        api: &'a Api,
        include_uploadable: bool,
    ) -> BoxFuture<'a, Result<Vec<File>>> {
        debug_assert!(include_uploadable || !self.allow_upload);

        async move {
            let get_subdirs = || async {
                let subdirs_resp = api
                    .api_as_json::<ApiData<Vec<ApiFileDirectory>>>(
                        &format!("files/?ParentID={}", self.id),
                        Method::GET,
                        None,
                    )
                    .await?;
                match subdirs_resp.data {
                    Some(subdirs) => future::join_all(
                        subdirs
                            .into_iter()
                            .filter(|s| include_uploadable || !s.allow_upload.unwrap_or(false))
                            .map(|s| DirectoryHandle {
                                id: s.id,
                                path: self.path.join(Path::new(&sanitise_filename(&s.name))),
                                allow_upload: s.allow_upload.unwrap_or(false),
                                /* last_updated: parse_time(&s.last_updated_date), */
                            })
                            .map(|dh| dh.load(api, include_uploadable)),
                    )
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>>>()
                    .map(|v| v.into_iter().flatten().collect::<Vec<_>>()),
                    None => Err("Invalid API response from server: type mismatch"),
                }
            };

            let get_files = || async {
                let files_resp = api
                    .api_as_json::<ApiData<Vec<ApiFileDirectory>>>(
                        &format!(
                            "files/{}/file{}",
                            self.id,
                            if self.allow_upload {
                                "?populate=Creator"
                            } else {
                                ""
                            }
                        ),
                        Method::GET,
                        None,
                    )
                    .await?;
                match files_resp.data {
                    Some(files) => Ok(files
                        .into_iter()
                        .map(|s| File {
                            id: s.id,
                            path: self.path.join({
                                let name_for_download =
                                    s.file_name.as_deref().unwrap_or(s.name.as_str());
                                if self.allow_upload {
                                    sanitise_filename(
                                        format!(
                                            "{} - {}",
                                            s.creator_name.as_deref().unwrap_or_else(|| "Unknown"),
                                            name_for_download
                                        )
                                        .as_str(),
                                    )
                                } else {
                                    sanitise_filename(name_for_download)
                                }
                            }),
                            last_updated: parse_time(&s.last_updated_date),
                        })
                        .collect::<Vec<_>>()),
                    None => Err("Invalid API response from server: type mismatch"),
                }
            };

            let (res_subdirs, res_files) = future::join(get_subdirs(), get_files()).await;
            let mut files = res_subdirs?;
            files.append(&mut res_files?);

            Ok(files)
        }
        .boxed()
    }
}

#[async_trait(?Send)]
impl SimpleDownloadableResource for File {
    fn path(&self) -> &Path {
        &self.path
    }

    fn get_last_updated(&self) -> SystemTime {
        self.last_updated
    }

    async fn get_download_url(&self, api: &Api) -> Result<Url> {
        let data = api
            .api_as_json::<ApiData<String>>(
                &format!("files/file/{}/downloadurl", self.id),
                Method::GET,
                None,
            )
            .await?;
        if let Some(url) = data.data {
            Ok(Url::parse(&url).map_err(|_| "Unable to parse URL")?)
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }
}

// Makes the paths of all the given files unique, based on the last updated time and the id.
// This function will also sort the files.
pub fn sort_and_make_all_paths_unique(files: &mut [File]) {
    files.sort_unstable_by(|file1, file2| {
        file1
            .path
            .cmp(&file2.path)
            .then_with(|| file1.last_updated.cmp(&file2.last_updated).reverse())
    });
    files.iter_mut().fold(Path::new(""), |path, file| {
        if path == file.path {
            file.path.set_file_name({
                let mut new_name = file.path.file_stem().map_or_else(OsString::new, |n| {
                    let mut new_name = n.to_owned();
                    new_name.push("_");
                    new_name
                });
                new_name.push(&file.id);
                file.path.extension().map(|e| {
                    new_name.push(".");
                    new_name.push(e);
                });
                new_name
            });
            path
        } else {
            file.path.as_ref()
        }
    });
}
