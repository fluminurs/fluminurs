use crate::api::{Api, ApiData, Data};
use crate::Result;
use serde::Deserialize;
use reqwest::Method;

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
pub struct Module {
    pub id: String,
    #[serde(rename = "name")]
    pub code: String,
    #[serde(rename = "courseName")]
    pub name: String,
    access: Access,
    pub term: String,
}

#[derive(Debug, Deserialize)]
pub struct Announcement {
    pub title: String,
    pub description: String
}

impl Module {
    pub fn is_teaching(&self) -> bool {
        let access = &self.access;
        access.full || access.create || access.update || access.delete || access.settings_read || access.settings_update
    }

    pub fn get_announcements(&self, api: &Api, archived: bool) -> Result<Vec<Announcement>> {
        let path = format!("/announcement/{}/{}?sortby=displayFrom%20ASC", if archived { "Archived" } else { "NonArchived" }, self.id);
        let api_data: ApiData = api.api_as_json(&path, Method::GET, None)?;
        if let Data::Announcements(announcements) = api_data.data {
            Ok(announcements)
        } else if let Data::Empty(_) = api_data.data {
            Ok(Vec::new())
        } else {
            Err("Invalid API response from server: type mismatch")
        }
    }
}
