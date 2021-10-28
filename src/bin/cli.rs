use std::collections::HashSet;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::{App, Arg};
use futures_util::{future, stream, StreamExt};
use serde::{Deserialize, Serialize};

use fluminurs::conferencing::ZoomRecording;
use fluminurs::file::File;
use fluminurs::module::Module;
use fluminurs::multimedia::ExternalVideo;
use fluminurs::multimedia::InternalVideo;
use fluminurs::resource::{
    sort_and_make_all_paths_unique, OverwriteMode, OverwriteResult, Resource,
};
use fluminurs::weblecture::WebLectureVideo;
use fluminurs::{Api, Result};

#[macro_use]
extern crate bitflags;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

#[derive(Serialize, Deserialize)]
struct Login {
    username: String,
    password: String,
}

bitflags! {
    struct ModuleTypeFlags: u8 {
        const TAKING = 0x01;
        const TEACHING = 0x02;
    }
}

fn flush_stdout() {
    io::stdout().flush().expect("Unable to flush stdout");
}

fn get_input(prompt: &str) -> String {
    let mut input = String::new();
    print!("{}", prompt);
    flush_stdout();
    io::stdin()
        .read_line(&mut input)
        .expect("Unable to get input");
    input.trim().to_string()
}

fn get_password(prompt: &str) -> String {
    print!("{}", prompt);
    flush_stdout();
    rpassword::read_password().expect("Unable to get non-echo input mode for password")
}

async fn print_announcements(api: &Api, modules: &[Module]) -> Result<()> {
    let module_announcements = future::join_all(
        modules
            .iter()
            .map(|module| module.get_announcements(api, false)),
    )
    .await;
    for (module, announcements) in modules.iter().zip(module_announcements) {
        let announcements = announcements?;
        println!("# {} {}", module.code, module.name);
        println!();
        for ann in announcements {
            println!("=== {} ===", ann.title);
            let stripped = ammonia::Builder::new()
                .tags(HashSet::new())
                .clean(&ann.description)
                .to_string();
            let decoded = htmlescape::decode_html(&stripped)
                .unwrap_or_else(|_| "Unable to decode HTML Entities".to_owned());
            println!("{}", decoded);
        }
        println!();
        println!();
    }
    Ok(())
}

async fn load_modules_files(
    api: &Api,
    modules: &[Module],
    include_uploadable_folders: ModuleTypeFlags,
) -> Result<Vec<File>> {
    let root_dirs_iter = modules
        .iter()
        .filter(|module| module.has_access())
        .map(|module| {
            (
                module.workbin_root(|code| Path::new(code).to_owned()),
                module.is_teaching(),
            )
        });

    let (files, errors) =
        future::join_all(root_dirs_iter.map(|(root_dir, is_teaching)| async move {
            root_dir
                .load(
                    api,
                    include_uploadable_folders.contains(if is_teaching {
                        ModuleTypeFlags::TEACHING
                    } else {
                        ModuleTypeFlags::TAKING
                    }),
                )
                .await
                .map(|mut files| {
                    // to avoid duplicate files from being corrupted,
                    // we append the id to duplicate resources
                    sort_and_make_all_paths_unique(&mut files);
                    files
                })
        }))
        .await
        .into_iter()
        .fold((vec![], vec![]), move |(mut ok, mut err), res| {
            match res {
                Ok(mut dir) => {
                    ok.append(&mut dir);
                }
                Err(e) => {
                    err.push(e);
                }
            }
            (ok, err)
        });
    for e in errors {
        println!("Failed loading module files: {}", e);
    }
    Ok(files)
}

async fn load_modules_multimedia(
    api: &Api,
    modules: &[Module],
) -> Result<(Vec<InternalVideo>, Vec<ExternalVideo>)> {
    let multimedias_iter = modules
        .iter()
        .filter(|module| module.has_access())
        .map(|module| module.multimedia_root(|code| Path::new(code).join(Path::new("Multimedia"))));

    let (internal_videos, external_videos, errors) =
        future::join_all(multimedias_iter.map(|multimedia| async move {
            multimedia.load(api).await.map(|(mut ivs, mut evs)| {
                // to avoid duplicate files from being corrupted,
                // we append the id to duplicate resources
                sort_and_make_all_paths_unique(&mut ivs);
                sort_and_make_all_paths_unique(&mut evs);
                (ivs, evs)
            })
        }))
        .await
        .into_iter()
        .fold(
            (vec![], vec![], vec![]),
            move |(mut internal_videos, mut external_videos, mut err), res| {
                match res {
                    Ok((mut iv, mut ev)) => {
                        internal_videos.append(&mut iv);
                        external_videos.append(&mut ev);
                    }
                    Err(e) => {
                        err.push(e);
                    }
                }
                (internal_videos, external_videos, err)
            },
        );

    for e in errors {
        println!("Failed loading module multimedia: {}", e);
    }
    Ok((internal_videos, external_videos))
}

async fn load_modules_weblectures(api: &Api, modules: &[Module]) -> Result<Vec<WebLectureVideo>> {
    let weblectures_iter = modules
        .iter()
        .filter(|module| module.has_access())
        .map(|module| {
            module.weblecture_root(|code| Path::new(code).join(Path::new("Web Lectures")))
        });

    let (files, errors) = future::join_all(weblectures_iter.map(|weblecture| async move {
        weblecture.load(api).await.map(|mut weblectures| {
            // to avoid duplicate files from being corrupted,
            // we append the id to duplicate resources
            sort_and_make_all_paths_unique(&mut weblectures);
            weblectures
        })
    }))
    .await
    .into_iter()
    .fold((vec![], vec![]), move |(mut ok, mut err), res| {
        match res {
            Ok(mut dir) => {
                ok.append(&mut dir);
            }
            Err(e) => {
                err.push(e);
            }
        }
        (ok, err)
    });

    for e in errors {
        println!("Failed loading module web lecture: {}", e);
    }
    Ok(files)
}

async fn load_modules_conferences(api: &Api, modules: &[Module]) -> Result<Vec<ZoomRecording>> {
    let conferences_iter = modules
        .iter()
        .filter(|module| module.has_access())
        .map(|module| {
            module.conferencing_root(|code| Path::new(code).join(Path::new("Conferences")))
        });

    let (zoom_recordings, errors) =
        future::join_all(conferences_iter.map(|conference| async move {
            conference.load(api).await.map(|mut conferences| {
                // to avoid duplicate files from being corrupted,
                // we append the id to duplicate resources
                sort_and_make_all_paths_unique(&mut conferences);
                conferences
            })
        }))
        .await
        .into_iter()
        .fold((vec![], vec![]), move |(mut ok, mut err), res| {
            match res {
                Ok(mut dir) => {
                    ok.append(&mut dir);
                }
                Err(e) => {
                    err.push(e);
                }
            }
            (ok, err)
        });

    for e in errors {
        println!("Failed loading module conferences: {}", e);
    }
    Ok(zoom_recordings)
}

fn list_resources<T: Resource>(resources: &[T]) {
    for resource in resources {
        println!("{}", resource.path().display())
    }
}

async fn download_resource<T: Resource>(
    api: &Api,
    file: &T,
    path: PathBuf,
    temp_path: PathBuf,
    overwrite_mode: OverwriteMode,
) {
    match file.download(api, &path, &temp_path, overwrite_mode).await {
        Ok(OverwriteResult::NewFile) => println!("Downloaded to {}", path.to_string_lossy()),
        Ok(OverwriteResult::AlreadyHave) => {}
        Ok(OverwriteResult::Skipped) => println!("Skipped {}", path.to_string_lossy()),
        Ok(OverwriteResult::Overwritten) => println!("Updated {}", path.to_string_lossy()),
        Ok(OverwriteResult::Renamed { renamed_path }) => println!(
            "Renamed {} to {}",
            path.to_string_lossy(),
            renamed_path.to_string_lossy()
        ),
        Err(e) => println!("Failed to download file: {}", e),
    }
}

async fn download_resources<T: Resource>(
    api: &Api,
    files: &[T],
    destination: &str,
    overwrite_mode: OverwriteMode,
    parallelism: usize,
) -> Result<()> {
    println!("Download to {}", destination);
    let dest_path = Path::new(destination);
    if !dest_path.is_dir() {
        return Err("Download destination does not exist or is not a directory");
    }

    stream::iter(files.iter())
        .map(|file| {
            let temp_path = dest_path
                .join(file.path().parent().unwrap())
                .join(make_temp_file_name(file.path().file_name().unwrap()));
            let real_path = dest_path.join(file.path());
            download_resource(api, file, real_path, temp_path, overwrite_mode)
        })
        .buffer_unordered(parallelism)
        .for_each(|_| future::ready(())) // do nothing, just complete the future
        .await;

    Ok(())
}

fn make_temp_file_name(name: &OsStr) -> OsString {
    let prepend = OsStr::new("~!");
    let mut res = OsString::with_capacity(prepend.len() + name.len());
    res.push(prepend);
    res.push(name);
    res
}

fn get_credentials(credential_file: &str) -> Result<(String, String)> {
    if let Ok(mut file) = fs::File::open(credential_file) {
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|_| "Unable to read credentials")?;
        if let Ok(login) = serde_json::from_str::<Login>(&content) {
            Ok((login.username, login.password))
        } else {
            println!("Corrupt credentials.json, deleting file...");
            fs::remove_file(Path::new(credential_file))
                .map_err(|_| "Unable to delete credential file")?;
            get_credentials(credential_file)
        }
    } else {
        let username = get_input("Username (include the nusstu\\ prefix): ");
        let password = get_password("Password: ");
        Ok((username, password))
    }
}

fn store_credentials(credential_file: &str, username: &str, password: &str) -> Result<()> {
    if confirm("Store credentials (WARNING: they are stored in plain text)? [y/n]") {
        let login = Login {
            username: username.to_owned(),
            password: password.to_owned(),
        };
        let serialised =
            serde_json::to_string(&login).map_err(|_| "Unable to serialise credentials")?;
        fs::write(credential_file, serialised)
            .map_err(|_| "Unable to write to credentials file")?;
    }
    Ok(())
}

fn confirm(prompt: &str) -> bool {
    print!("{} ", prompt);
    flush_stdout();
    let mut answer = String::new();
    while answer != "y" && answer != "n" {
        answer = get_input("");
        answer.make_ascii_lowercase();
    }
    answer == "y"
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "with-env-logger")]
    env_logger::init();

    let matches = App::new(PKG_NAME)
        .version(VERSION)
        .author(&*format!("{} and contributors", clap::crate_authors!(", ")))
        .about(DESCRIPTION)
        .arg(Arg::with_name("announcements").long("announcements"))
        .arg(Arg::with_name("files").long("files"))
        .arg(
            Arg::with_name("download")
                .long("download-to")
                .takes_value(true),
        )
        .arg(Arg::with_name("list-multimedia").long("list-multimedia"))
        .arg(
            Arg::with_name("download-multimedia")
                .long("download-multimedia-to")
                .takes_value(true),
        )
        .arg(Arg::with_name("list-weblectures").long("list-weblectures"))
        .arg(
            Arg::with_name("download-weblectures")
                .long("download-weblectures-to")
                .takes_value(true),
        )
        .arg(Arg::with_name("list-conferences").long("list-conferences"))
        .arg(
            Arg::with_name("download-conferences")
                .long("download-conferences-to")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("credential-file")
                .long("credential-file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("include-uploadable")
                .long("include-uploadable-folders")
                .takes_value(true)
                .min_values(0)
                .max_values(u64::max_value())
                .possible_values(&["taking", "teaching", "all"]),
        )
        .arg(
            Arg::with_name("updated")
                .long("updated")
                .takes_value(true)
                .value_name("action-on-updated-files")
                .possible_values(&["skip", "overwrite", "rename"])
                .number_of_values(1)
                .default_value("skip"),
        )
        .arg(
            Arg::with_name("term")
                .long("term")
                .takes_value(true)
                .value_name("term")
                .number_of_values(1),
        )
        .arg(
            Arg::with_name("modules")
                .long("modules")
                .takes_value(true)
                .value_name("modules")
                .min_values(1)
                .max_values(u64::MAX),
        )
        .arg(
            Arg::with_name("ffmpeg")
                .long("ffmpeg")
                .takes_value(true)
                .value_name("ffmpeg-path")
                .number_of_values(1)
                .default_value("ffmpeg")
                .help("Path to ffmpeg executable for downloading multimedia"),
        )
        .get_matches();
    let credential_file = matches
        .value_of("credential-file")
        .unwrap_or("login.json")
        .to_owned();
    let do_announcements = matches.is_present("announcements");
    let do_files = matches.is_present("files");
    let download_destination = matches.value_of("download").map(|s| s.to_owned());
    let do_multimedia = matches.is_present("list-multimedia");
    let multimedia_download_destination = matches
        .value_of("download-multimedia")
        .map(|s| s.to_owned());
    let do_weblectures = matches.is_present("list-weblectures");
    let weblectures_download_destination = matches
        .value_of("download-weblectures")
        .map(|s| s.to_owned());
    let do_conferences = matches.is_present("list-conferences");
    let conferences_download_destination = matches
        .value_of("download-conferences")
        .map(|s| s.to_owned());
    let include_uploadable_folders = matches
        .values_of("include-uploadable")
        .map(|values| {
            let include_flags = values
                .fold(Ok(ModuleTypeFlags::empty()), |acc, s| {
                    acc.and_then(|flag| match s.to_lowercase().as_str() {
                        "taking" => Ok(flag | ModuleTypeFlags::TAKING),
                        "teaching" => Ok(flag | ModuleTypeFlags::TEACHING),
                        "all" => Ok(flag | ModuleTypeFlags::all()),
                        _ => Err("Invalid module type"),
                    })
                })
                .expect("Unable to parse parameters of include-uploadable");
            if include_flags.is_empty() {
                ModuleTypeFlags::all()
            } else {
                include_flags
            }
        })
        .unwrap_or_else(ModuleTypeFlags::empty);
    let overwrite_mode = matches
        .value_of("updated")
        .map(|s| match s.to_lowercase().as_str() {
            "skip" => OverwriteMode::Skip,
            "overwrite" => OverwriteMode::Overwrite,
            "rename" => OverwriteMode::Rename,
            _ => panic!("Unable to parse parameter of overwrite_mode"),
        })
        .unwrap_or(OverwriteMode::Skip);
    let specified_term = matches.value_of("term").map(|s| {
        if s.len() == 4 && s.chars().all(char::is_numeric) {
            s.to_owned()
        } else {
            panic!("Invalid input term")
        }
    });
    let specified_modules = matches
        .values_of("modules")
        .map(|it| it.collect::<Vec<&str>>());

    let (username, password) =
        get_credentials(&credential_file).expect("Unable to get credentials");

    let mut api = Api::with_login(&username, &password)
        .await?
        .with_ffmpeg(matches.value_of("ffmpeg").unwrap_or("ffmpeg").to_owned());
    if !Path::new(&credential_file).exists() {
        match store_credentials(&credential_file, &username, &password) {
            Ok(_) => (),
            Err(e) => println!("Failed to store credentials: {}", e),
        }
    }

    let name = api.name().await?;
    println!("Hi {}!", name);
    let all_modules = api.modules(specified_term).await?;
    let modules = if let Some(module_codes) = specified_modules {
        for module_code in &module_codes {
            if !all_modules.iter().any(|m| m.code == *module_code) {
                panic!("Module {} is not available", module_code);
            }
        }
        let filtered_modules = all_modules
            .into_iter()
            .filter(|m| module_codes.iter().any(|code| m.code.as_str() == *code))
            .collect::<Vec<Module>>();
        println!("Selected modules:");
        for module in &filtered_modules {
            println!("- {} {}", module.code, module.name);
        }
        filtered_modules
    } else {
        println!("You are taking:");
        for module in all_modules.iter().filter(|m| m.is_taking()) {
            println!("- {} {}", module.code, module.name);
        }
        println!("You are teaching:");
        for module in all_modules.iter().filter(|m| m.is_teaching()) {
            println!("- {} {}", module.code, module.name);
        }
        all_modules
    };

    if do_announcements {
        print_announcements(&api, &modules).await?;
    }

    if do_files || download_destination.is_some() {
        let module_file = load_modules_files(&api, &modules, include_uploadable_folders).await?;

        if do_files {
            list_resources(&module_file);
        }

        if let Some(destination) = download_destination {
            download_resources(&api, &module_file, &destination, overwrite_mode, 64).await?;
        }
    }

    if do_multimedia || multimedia_download_destination.is_some() {
        let (module_internal_multimedia, module_external_multimedia) =
            load_modules_multimedia(&api, &modules).await?;

        if do_multimedia {
            list_resources(&module_internal_multimedia);
            list_resources(&module_external_multimedia);
        }

        if let Some(destination) = multimedia_download_destination {
            // We download internal and external multimedia separately
            // because we don't want the download slots to be shared between them
            // (since internal multimedia is from LumiNUS but external multimedia is from Panopto)
            let (internal_result, external_result) = future::join(
                download_resources(
                    &api,
                    &module_internal_multimedia,
                    &destination,
                    overwrite_mode,
                    4,
                ),
                download_resources(
                    &api,
                    &module_external_multimedia,
                    &destination,
                    overwrite_mode,
                    4,
                ),
            )
            .await;
            internal_result?;
            external_result?;
        }
    }

    if do_weblectures || weblectures_download_destination.is_some() {
        let module_weblectures = load_modules_weblectures(&api, &modules).await?;

        if do_weblectures {
            list_resources(&module_weblectures);
        }

        if let Some(destination) = weblectures_download_destination {
            download_resources(&api, &module_weblectures, &destination, overwrite_mode, 4).await?;
        }
    }

    if do_conferences || conferences_download_destination.is_some() {
        let module_conferences = load_modules_conferences(&api, &modules).await?;

        if do_conferences {
            list_resources(&module_conferences);
        }

        if let Some(destination) = conferences_download_destination {
            if !module_conferences.is_empty() {
                match api.login_zoom().await {
                    Err(e) => {
                        println!("Failed to log in to Zoom: {}", e);
                    }
                    Ok(_) => {
                        println!("Logged in to Zoom");
                        download_resources(
                            &api,
                            &module_conferences,
                            &destination,
                            overwrite_mode,
                            4,
                        )
                        .await?;
                    }
                }
            }
        }
    }

    Ok(())
}
