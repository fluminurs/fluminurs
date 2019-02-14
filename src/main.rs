type Result<T> = std::result::Result<T, &'static str>;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const AUTHOR: &str = env!("CARGO_PKG_AUTHORS");
const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

mod api;

use crate::api::module::{File, Module};
use crate::api::Api;
use clap::{App, Arg};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Write;
use std::path::Path;

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

fn print_files(file: &File, api: &Api, prefix: &str) -> Result<bool> {
    if file.is_directory {
        for mut child in file
            .children
            .clone()
            .ok_or("children must be preloaded")?
            .into_iter()
        {
            child.load_children(api)?;
            print_files(&child, api, &format!("{}/{}", prefix, file.name))?;
        }
    } else {
        println!(
            "{}/{}, download url: {}",
            prefix,
            file.name,
            file.get_download_url(api)?
        );
    }
    Ok(true)
}

fn print_announcements(api: &Api, modules: &[Module]) -> Result<bool> {
    for module in modules {
        println!("# {} {}", module.code, module.name);
        println!();
        for announcement in module.get_announcements(&api, false)? {
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
    Ok(true)
}

fn list_files(api: &Api, modules: &[Module]) -> Result<bool> {
    for module in modules {
        print_files(&module.as_file(&api, true)?, &api, "")?;
    }
    Ok(true)
}

fn download_file(api: &Api, file: &File, path: &Path) -> Result<bool> {
    let destination = path.join(file.name.to_owned());
    if file.is_directory {
        fs::create_dir_all(destination.to_owned()).map_err(|_| "Unable to create directory")?;

        for mut child in file
            .children
            .clone()
            .ok_or("children must be preloaded")?
            .into_iter()
        {
            child.load_children(api)?;
            download_file(api, &child, &destination)?;
        }
    } else {
        file.download(api, path)?;
        println!("Downloaded to {}", destination.to_string_lossy());
    }
    Ok(true)
}

fn download_files(api: &Api, modules: &[Module], destination: &str) -> Result<bool> {
    println!("Download to {}", destination);
    let path = Path::new(destination);
    if !path.is_dir() {
        return Err("Download destination does not exist or is not a directory");
    }
    for module in modules {
        println!("## {}", module.code);
        println!();
        download_file(api, &module.as_file(api, true)?, &path)?;
    }
    Ok(true)
}

fn main() {
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
        .get_matches();
    let username = get_input("Username: ");
    let password = get_password("Password: ");
    let api = Api::with_login(&username, &password).expect("Unable to login");
    println!("Hi {}!", api.name().expect("Unable to read name"));
    let modules = api.modules(true).expect("Unable to retrieve modules");
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
        download_files(&api, &modules, &destination).expect("Failed during downloading");
    }
}
