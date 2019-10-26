use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::{App, Arg};
use futures_util::future;
use serde::{Deserialize, Serialize};
use tokio;

use crate::api::module::{File, Module};
use crate::api::Api;

type Error = &'static str;
type Result<T> = std::result::Result<T, Error>;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const AUTHOR: &str = env!("CARGO_PKG_AUTHORS");
const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

mod api;

#[derive(Serialize, Deserialize)]
struct Login {
    username: String,
    password: String,
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

fn print_files(file: &File, prefix: &str) {
    if file.is_directory() {
        for child in file.children().expect("Children must have been loaded") {
            print_files(&child, &format!("{}/{}", prefix, file.name()));
        }
    } else {
        println!("{}/{}", prefix, file.name());
    }
}

async fn print_announcements(api: &Api, modules: &[Module]) -> Result<()> {
    let apic = api.clone();

    let module_announcements = future::join_all(
        modules
            .iter()
            .map(|module| module.get_announcements(&apic, false)),
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

async fn load_modules_files(api: &Api, modules: &[Module]) -> Result<Vec<File>> {
    let apic = api.clone();

    let files = modules.iter().map(Module::as_file).collect::<Vec<_>>();

    let errors = future::join_all(files.iter().map(|file| file.load_all_children(&apic)))
        .await
        .into_iter()
        .filter(Result::is_err);
    for e in errors {
        println!("Failed loading module files: {}", e.unwrap_err());
    }
    Ok(files)
}

async fn list_files(api: &Api, modules: &[Module]) -> Result<()> {
    let files = load_modules_files(api, modules).await?;
    for file in files {
        print_files(&file, "");
    }
    Ok(())
}

async fn download_file(api: &Api, file: File, path: PathBuf) {
    match file.download(api.clone(), &path).await {
        Ok(true) => println!("Downloaded to {}", path.to_string_lossy()),
        Ok(false) => (),
        Err(e) => println!("Failed to download file: {}", e),
    }
}

async fn download_files(api: &Api, modules: &[Module], destination: &str) -> Result<()> {
    println!("Download to {}", destination);
    let path = Path::new(destination).to_owned();
    if !path.is_dir() {
        return Err("Download destination does not exist or is not a directory");
    }

    let files = load_modules_files(api, modules).await?;

    let mut directories = files
        .into_iter()
        .zip(std::iter::repeat(path))
        .collect::<Vec<_>>();
    let mut files: Vec<(File, PathBuf)> = vec![];

    while let Some((file, path)) = directories.pop() {
        let path = path.join(file.name());
        if file.is_directory() {
            directories.append(
                &mut file
                    .children()
                    .expect("Children should have been loaded")
                    .into_iter()
                    .map(|child| (child, path.clone()))
                    .collect(),
            );
        } else {
            files.push((file, path));
        }
    }
    future::join_all(
        files
            .into_iter()
            .map(|(file, path)| download_file(api, file, path)),
    )
    .await;
    Ok(())
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
    let matches = App::new(PKG_NAME)
        .version(VERSION)
        .author(AUTHOR)
        .about(DESCRIPTION)
        .arg(Arg::with_name("announcements").long("announcements"))
        .arg(Arg::with_name("files").long("files"))
        .arg(
            Arg::with_name("download")
                .long("download-to")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("credential-file")
                .long("credential-file")
                .takes_value(true),
        )
        .get_matches();
    let credential_file = matches
        .value_of("credential-file")
        .unwrap_or("login.json")
        .to_owned();
    let do_announcements = matches.is_present("announcements");
    let do_files = matches.is_present("files");
    let download_destination = matches.value_of("download").map(|s| s.to_owned());

    let (username, password) =
        get_credentials(&credential_file).expect("Unable to get credentials");

    let api = Api::with_login(&username, &password).await?;
    if !Path::new(&credential_file).exists() {
        match store_credentials(&credential_file, &username, &password) {
            Ok(_) => (),
            Err(e) => println!("Failed to store credentials: {}", e),
        }
    }

    let name = api.name().await?;
    println!("Hi {}!", name);
    let modules = api.modules(true).await?;
    println!("You are taking:");
    for module in modules.iter().filter(|m| m.is_taking()) {
        println!("- {} {}", module.code, module.name);
    }
    println!("You are teaching:");
    for module in modules.iter().filter(|m| m.is_teaching()) {
        println!("- {} {}", module.code, module.name);
    }

    if do_announcements {
        print_announcements(&api, &modules).await?;
    }

    if do_files {
        list_files(&api, &modules).await?;
    }

    if let Some(destination) = download_destination {
        download_files(&api, &modules, &destination).await?;
    }

    Ok(())
}
