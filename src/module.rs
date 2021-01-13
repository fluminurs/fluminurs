use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use futures_util::future;
use reqwest::{Method, Url};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::{Api, ApiData, Data, Error, Result};

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
    access: Option<Access>,
    pub term: String,
}

impl Module {
    pub fn is_teaching(&self) -> bool {
        self.access
            .as_ref()
            .map(|access| {
                access.full
                    || access.create
                    || access.update
                    || access.delete
                    || access.settings_read
                    || access.settings_update
            })
            .unwrap_or(false)
    }

    pub fn is_taking(&self) -> bool {
        !self.is_teaching()
    }

    pub fn has_access(&self) -> bool {
        self.access.is_some()
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
                last_updated: std::time::UNIX_EPOCH,
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
    last_updated: SystemTime,
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

fn parse_time(time: &str) -> SystemTime {
    SystemTime::from(
        chrono::DateTime::<chrono::FixedOffset>::parse_from_rfc3339(&time)
            .expect("Failed to parse last updated time"),
    )
}

#[derive(Copy, Clone)]
pub enum OverwriteMode {
    Skip,
    Overwrite,
    Rename,
}

pub enum OverwriteResult {
    NewFile,
    AlreadyHave,
    Skipped,
    Overwritten,
    Renamed { renamed_path: PathBuf },
}

enum RetryableError {
    Retry(Error),
    Fail(Error),
}

type RetryableResult<T> = std::result::Result<T, RetryableError>;

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

    pub async fn load_children(&self, api: &Api, include_uploadable: bool) -> Result<()> {
        debug_assert!(include_uploadable || !self.inner.allow_upload);

        let apic = api.clone();
        if !self.inner.is_directory {
            return self
                .inner
                .children
                .write()
                .map(|mut ptr| {
                    *ptr = Some(Vec::new());
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
                .filter(|s| include_uploadable || !s.allow_upload.unwrap_or(false))
                .map(|s| File {
                    inner: Arc::new(FileInner {
                        id: s.id,
                        name: sanitise_filename(s.name),
                        is_directory: true,
                        children: RwLock::new(None),
                        allow_upload: s.allow_upload.unwrap_or(false),
                        last_updated: parse_time(&s.last_updated_date),
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
                                format!(
                                    "{} - ",
                                    s.creator_name.unwrap_or_else(|| "Unknown".to_string())
                                )
                            } else {
                                "".to_string()
                            },
                            s.name
                        )),
                        is_directory: false,
                        children: RwLock::new(Some(Vec::new())),
                        allow_upload: false,
                        last_updated: parse_time(&s.last_updated_date),
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

    pub async fn load_all_children(&self, api: &Api, include_uploadable: bool) -> Result<()> {
        let apic = api.clone();
        self.load_children(api, include_uploadable).await?;

        let mut files = vec![self.clone()];
        loop {
            for res in future::join_all(
                files
                    .iter()
                    .map(|file| file.load_children(&apic, include_uploadable)),
            )
            .await
            {
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

    async fn prepare_path(
        &self,
        path: &Path,
        overwrite: OverwriteMode,
    ) -> Result<(bool, OverwriteResult)> {
        let metadata = tokio::fs::metadata(path).await;
        if let Err(e) = metadata {
            return match e.kind() {
                std::io::ErrorKind::NotFound => Ok((true, OverwriteResult::NewFile)), // do download, because file does not already exist
                std::io::ErrorKind::PermissionDenied => {
                    Err("Permission denied when retrieving file metadata")
                }
                _ => Err("Unable to retrieve file metadata"),
            };
        }
        let old_time = metadata
            .unwrap()
            .modified()
            .map_err(|_| "File system does not support last modified time")?;
        if self.inner.last_updated <= old_time {
            Ok((false, OverwriteResult::AlreadyHave)) // don't download, because we already have updated file
        } else {
            match overwrite {
                OverwriteMode::Skip => Ok((false, OverwriteResult::Skipped)), // don't download, because user wants to skip updated files
                OverwriteMode::Overwrite => Ok((true, OverwriteResult::Overwritten)), // do download, because user wants to overwrite updated files
                OverwriteMode::Rename => {
                    let mut new_stem = path
                        .file_stem()
                        .expect("File does not have name")
                        .to_os_string();
                    let date = chrono::DateTime::<chrono::Local>::from(old_time).date();
                    use chrono::Datelike;
                    new_stem.push(format!(
                        "_autorename_{:04}-{:02}-{:02}",
                        date.year(),
                        date.month(),
                        date.day()
                    ));
                    let path_extension = path.extension();
                    let mut i = 0;
                    let mut suffixed_stem = new_stem.clone();
                    let renamed_path = loop {
                        let renamed_path_without_ext = path.with_file_name(suffixed_stem);
                        let renamed_path = if let Some(ext) = path_extension {
                            renamed_path_without_ext.with_extension(ext)
                        } else {
                            renamed_path_without_ext
                        };
                        if !renamed_path.exists() {
                            break renamed_path;
                        }
                        i += 1;
                        suffixed_stem = new_stem.clone();
                        suffixed_stem.push(format!("_{}", i));
                    };
                    tokio::fs::rename(path, renamed_path.clone())
                        .await
                        .map_err(|_| "Failed renaming existing file")?;
                    Ok((true, OverwriteResult::Renamed { renamed_path })) // do download, because we renamed the old file
                }
            }
        }
    }

    pub async fn download(
        &self,
        api: Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult> {
        let (should_download, result) = self.prepare_path(destination, overwrite).await?;
        if should_download {
            let download_url = self.get_download_url(api.clone()).await?;
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|_| "Unable to create directory")?;
            };
            Self::infinite_retry_download(&api, download_url, destination, temp_destination)
                .await?;
            // Note: We should actually manually set the last updated time on the disk to the time fetched from server, otherwise there might be situations where we will miss an updated file.
        }
        Ok(result)
    }

    async fn infinite_retry_download(
        api: &Api,
        download_url: reqwest::Url,
        destination: &Path,
        temp_destination: &Path,
    ) -> Result<()> {
        loop {
            let mut file = tokio::fs::File::create(temp_destination)
                .await
                .map_err(|e| {
                    println!("{} {}", temp_destination.to_str().unwrap(), e);
                    "Unable to open temporary file"
                })?;
            match Self::download_chunks(&api, download_url.clone(), &mut file).await {
                Ok(_) => {
                    tokio::fs::rename(temp_destination, destination)
                        .await
                        .map_err(|_| "Unable to move temporary file")?;
                    break;
                }
                Err(err) => {
                    tokio::fs::remove_file(temp_destination)
                        .await
                        .map_err(|_| "Unable to delete temporary file")?;
                    match err {
                        RetryableError::Retry(_) => { /* retry */ }
                        RetryableError::Fail(err) => {
                            Err(err)?;
                        }
                    }
                }
            };
        }
        Ok(())
    }

    async fn download_chunks(
        api: &Api,
        download_url: reqwest::Url,
        file: &mut tokio::fs::File,
    ) -> RetryableResult<()> {
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
