use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future;
use futures_util::future::{BoxFuture, FutureExt};
use reqwest::{Method, Url};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::resource;
use crate::resource::{OverwriteMode, OverwriteResult, Resource, RetryableError, RetryableResult};
use crate::util::{parse_time, sanitise_filename};
use crate::{Api, ApiData, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiFileDirectory {
    id: String,
    name: String,
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
                            path: self.path.join(if self.allow_upload {
                                sanitise_filename(
                                    format!(
                                        "{} - {}",
                                        s.creator_name.as_deref().unwrap_or_else(|| "Unknown"),
                                        s.name.as_str()
                                    )
                                    .as_str(),
                                )
                            } else {
                                sanitise_filename(s.name.as_str())
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
impl Resource for File {
    fn path(&self) -> &Path {
        &self.path
    }

    async fn download(
        &self,
        api: &Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult> {
        resource::do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.last_updated,
            move |api| self.get_download_url(api),
            move |api, url, temp_destination| Self::download_chunks(api, url, temp_destination),
        )
        .await
    }
}

impl File {
    pub async fn get_download_url(&self, api: &Api) -> Result<Url> {
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

    async fn download_chunks(
        api: &Api,
        download_url: reqwest::Url,
        temp_destination: &Path,
    ) -> RetryableResult<()> {
        let mut file = tokio::fs::File::create(temp_destination)
            .await
            .map_err(|_| RetryableError::Fail("Unable to open temporary file"))?;
        let mut res = api
            .get_client()
            .get(download_url)
            .send()
            .await
            .map_err(|_| RetryableError::Retry("Failed during download"))?;
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|_| RetryableError::Retry("Failed during streaming"))?
            .as_deref()
        {
            file.write_all(chunk)
                .await
                .map_err(|_| RetryableError::Fail("Failed writing to disk"))?;
        }
        Ok(())
    }
}
