use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use futures_util::future::Future;
use reqwest::Url;
use tokio::io::AsyncWriteExt;

use crate::{Api, Error, Result};

#[async_trait(?Send)]
pub trait Resource {
    fn path(&self) -> &Path;
    async fn download(
        &self,
        api: &Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult>;
}

#[async_trait(?Send)]
pub trait SimpleDownloadableResource {
    fn path(&self) -> &Path;
    fn get_last_updated(&self) -> SystemTime;
    async fn get_download_url(&self, api: &Api) -> Result<Url>;
}

#[async_trait(?Send)]
impl<T: SimpleDownloadableResource> Resource for T {
    fn path(&self) -> &Path {
        self.path()
    }

    async fn download(
        &self,
        api: &Api,
        destination: &Path,
        temp_destination: &Path,
        overwrite: OverwriteMode,
    ) -> Result<OverwriteResult> {
        do_retryable_download(
            api,
            destination,
            temp_destination,
            overwrite,
            self.get_last_updated(),
            move |api| self.get_download_url(api),
            move |api, url, temp_destination| download_chunks(api, url, temp_destination),
        )
        .await
    }
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

pub enum RetryableError {
    Retry(Error),
    Fail(Error),
}

pub type RetryableResult<T> = std::result::Result<T, RetryableError>;

pub async fn do_retryable_download<
    'a,
    F1: FnOnce(&'a Api) -> Fut1 + 'a,
    Fut1: Future<Output = Result<C>>,
    F2: Fn(&'a Api, C, &'a Path) -> Fut2 + 'a,
    Fut2: Future<Output = RetryableResult<()>>,
    C: Clone,
>(
    api: &'a Api,
    destination: &Path,
    temp_destination: &'a Path,
    overwrite: OverwriteMode,
    last_updated: SystemTime,
    before_download_file: F1,
    download_file: F2,
) -> Result<OverwriteResult> {
    let (should_download, result) = prepare_path(destination, overwrite, last_updated).await?;
    if should_download {
        let before_download_data = before_download_file(api).await?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|_| "Unable to create directory")?;
        };
        infinite_retry_download(
            api,
            before_download_data,
            destination,
            temp_destination,
            download_file,
        )
        .await?;

        // set the last modified time manually to the time we got from the server,
        // so that in case our local machine has unsynced time, or the file got updated while we are downloading it,
        // we will be able to update the file the next time we attempt to download it
        filetime::set_file_mtime(
            destination,
            filetime::FileTime::from_system_time(last_updated),
        )
        .map_err(|_| "Unable to set last modified time")?;
    }
    Ok(result)
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

async fn prepare_path(
    path: &Path,
    overwrite: OverwriteMode,
    last_updated: SystemTime,
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
    if last_updated <= old_time {
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

async fn infinite_retry_download<
    'a,
    F: Fn(&'a Api, C, &'a Path) -> Fut + 'a,
    Fut: Future<Output = RetryableResult<()>>,
    C: Clone,
>(
    api: &'a Api,
    before_download_data: C,
    destination: &Path,
    temp_destination: &'a Path,
    download_file: F,
) -> Result<()> {
    loop {
        match download_file(api, before_download_data.clone(), temp_destination).await {
            Ok(_) => {
                tokio::fs::rename(temp_destination, destination)
                    .await
                    .map_err(|_| "Unable to move temporary file")?;
                break;
            }
            Err(err) => {
                let success = tokio::fs::remove_file(temp_destination).await.is_ok();
                match err {
                    RetryableError::Retry(_) => {
                        if !success {
                            return Err("Unable to delete temporary file");
                        }
                        /* retry */
                    }
                    RetryableError::Fail(err) => {
                        // return the underlying error (perhaps explaining why the file can't be created)
                        return Err(err);
                    }
                }
            }
        };
    }
    Ok(())
}
