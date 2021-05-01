use std::path::PathBuf;

use reqwest::Method;
use serde::Deserialize;

use crate::file::DirectoryHandle;
use crate::multimedia::MultimediaHandle;
use crate::weblecture::WebLectureHandle;
use crate::util::sanitise_filename;
use crate::{Api, ApiData, Result};

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
        let api_data = api
            .api_as_json::<ApiData<Vec<Announcement>>>(&path, Method::GET, None)
            .await?;
        if let Some(announcements) = api_data.data {
            Ok(announcements)
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }

    pub fn workbin_root<F: FnOnce(&str) -> PathBuf>(&self, make_path: F) -> DirectoryHandle {
        DirectoryHandle::new(self.id.clone(), make_path(&sanitise_filename(&self.code)))
    }

    pub fn multimedia_root<F: FnOnce(&str) -> PathBuf>(&self, make_path: F) -> MultimediaHandle {
        MultimediaHandle::new(self.id.clone(), make_path(&sanitise_filename(&self.code)))
    }

    pub fn weblecture_root<F: FnOnce(&str) -> PathBuf>(&self, make_path: F) -> WebLectureHandle {
        WebLectureHandle::new(self.id.clone(), make_path(&sanitise_filename(&self.code)))
    }
}
