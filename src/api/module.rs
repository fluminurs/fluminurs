use serde::Deserialize;

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub struct Module {
    id: String,
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
        access.full || access.create || access.update || access.delete || access.settings_read || access.settings_update
    }
}
