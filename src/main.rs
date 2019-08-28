use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::path::Path;

use clap::{App, Arg};
use futures::Future;
use serde::{Deserialize, Serialize};
use tokio;
use tokio_executor;

use crate::api::Api;
use crate::api::module::{File, Module};

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

fn print_files(file: &File, api: &Api, prefix: &str) -> Result<()> {
    if file.inner.is_directory {
        for child in file.inner.children.read()
            .map_err(|_| "Failed to acquire children read lock")?
            .clone().ok_or("children must be preloaded")?.into_iter() {
            child.load_children(api.clone()).wait().expect("Failed to load children");
            print_files(&child, api, &format!("{}/{}", prefix, file.inner.name))?;
        }
    } else {
        println!("{}/{}", prefix, file.inner.name);
    }
    Ok(())
}

fn print_announcements(api: &Api, modules: &[Module]) -> Result<()> {
    for module in modules {
        println!("# {} {}", module.code, module.name);
        println!();
        for announcement in module.get_announcements(&api, false)
            .wait().expect("Failed to get announcements") {
            println!("=== {} ===", announcement.title);
            let stripped = ammonia::Builder::new()
                .tags(HashSet::new())
                .clean(&announcement.description)
                .to_string();
            let decoded =
                htmlescape::decode_html(&stripped).map_err(|_| "Unable to decode HTML Entities")?;
            println!("{}", decoded);
        }
        println!();
        println!();
    }
    Ok(())
}

fn list_files(api: &Api, modules: &[Module]) -> Result<()> {
    for module in modules {
        let file = module.as_file();
        file.load_children(api.clone()).wait().expect("Failed to load children");
        print_files(&file, api, "")?;
    }
    Ok(())
}

fn download_file(api: &Api, file: &File, path: &Path) -> Result<()> {
    let destination = path.join(file.inner.name.to_owned());
    if file.inner.is_directory {
        fs::create_dir_all(destination.to_owned()).map_err(|_| "Unable to create directory")?;
        for child in file.inner.children.read()
            .map_err(|_| "Failed to acquire children read lock")?
            .clone().ok_or("children must be preloaded")?.into_iter() {
            let api_clone = api.clone();
            let destination_clone = destination.clone();
            tokio::spawn(child.load_children(api.clone())
                    .map_err(|e| {
                        println!("Failed to load children: {}", e);
                    })
                    .and_then(move |_| download_file(&api_clone, &child, &destination_clone)
                        .map_err(|e| {
                            println!("Failed to download file: {}", e);
                        })));
        }
    } else {
        tokio::spawn(file.download(api.clone(), path)
            .map(move |result| if result {
                println!("Downloaded to {}", destination.to_string_lossy());
            })
            .map_err(|e| {
                println!("Failed to download file: {}", e);
            }));
    }
    Ok(())
}

fn download_files(api: &Api, modules: &[Module], destination: &str) -> Result<()> {
    println!("Download to {}", destination);
    let path = Path::new(destination).to_owned();
    if !path.is_dir() {
        return Err("Download destination does not exist or is not a directory");
    }
    for module in modules {
        let file = module.as_file();
        let api_clone = api.clone();
        let path_clone = path.clone();
        tokio::spawn(file.load_children(api.clone())
            .map_err(|e| {
                println!("Failed to load children: {}", e);
            })
            .and_then(move |_| download_file(&api_clone, &file, &path_clone)
                .map_err(|e| {
                    println!("Failed to download file: {}", e);
                })));
    }
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

fn store_credentials(credential_file: &str, username: &str, password: &str) -> Result<bool> {
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
    Ok(true)
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

fn executor_main() {
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
    let credential_file = matches.value_of("credential-file").unwrap_or("login.json");
    let (username, password) = get_credentials(credential_file).expect("Unable to get credentials");
    let api = Api::with_login(&username, &password).wait()
        .map_err(|e| println!("Error: {}", e))
        .unwrap();
    if !Path::new(credential_file).exists() {
        store_credentials(&credential_file, &username, &password)
            .expect("Unable to store credentials");
    }
    println!("Hi {}!", api.name().wait().expect("Unable to read name"));
    let modules = api.modules(true).wait().expect("Unable to retrieve modules");
    println!("You are taking:");
    for module in modules.iter().filter(|m| m.is_taking()) {
        println!("- {} {}", module.code, module.name);
    }
    println!("You are teaching:");
    for module in modules.iter().filter(|m| m.is_teaching()) {
        println!("- {} {}", module.code, module.name);
    }
    if matches.is_present("announcements") {
        print_announcements(&api, &modules).expect("Unable to list announcements");
    }
    if matches.is_present("files") {
        list_files(&api, &modules).expect("Unable to list files");
    }
    if let Some(destination) = matches.value_of("download") {
        let destination = destination.to_owned();
        download_files(&api, &modules, &destination).expect("Failed during downloading");
    }
}

fn main() {
    // FIXME HACK HACK HACK
    // Ideally we should use tokio::run to run a future that spawns other futures
    // But because I'm too lazy to properly re-write main(), I just substituted in wait()
    // Because we cannot use wait() on an executor (we'll cause threadpool starvation),
    // I have to manually fix-up an execution context so that libraries that spawn using
    // tokio::spawn (which is the proper way to spawn a future) work.
    let rt = tokio::runtime::Runtime::new().expect("Failed to start Tokio runtime");
    tokio_executor::with_default(&mut rt.executor(),
        &mut tokio_executor::enter().expect("Failed to enter execution context"),
        |_| executor_main());
    rt.shutdown_on_idle().wait().expect("Failed to shutdown runtime");
}
