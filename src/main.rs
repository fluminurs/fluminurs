type Result<T> = std::result::Result<T, &'static str>;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const AUTHOR: &str = env!("CARGO_PKG_AUTHORS");
const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

mod api;

use api::Api;
use api::module::{File, Module};
use clap::{Arg, App};
use std::collections::HashSet;
use std::io;
use std::io::Write;

fn flush_stdout() {
    io::stdout().flush().expect("Unable to flush stdout");
}

fn get_input(prompt: &str) -> String {
    let mut input = String::new();
    print!("{}", prompt);
    flush_stdout();
    io::stdin().read_line(&mut input).expect("Unable to get input");
    input.trim().to_string()
}

fn get_password(prompt: &str) -> String {
    print!("{}", prompt);
    flush_stdout();
    rpassword::read_password().expect("Unable to get non-echo input mode for password")
}

fn print_files(file: &File, api: &Api, prefix: &str) -> Result<bool> {
    if file.is_directory {
        for mut child in file.children.clone().ok_or("children must be preloaded")?.into_iter() {
            child.load_children(api)?;
            print_files(&child, api, &format!("{}/{}", prefix, file.name))?;
        }
    } else {
        println!("{}/{}, download url: {}", prefix, file.name, file.get_download_url(api)?);
    }
    Ok(true)
}

fn print_announcements(api: &Api, modules: &Vec<Module>) -> Result<bool> {
    for module in modules {
        println!("# {} {}", module.code, module.name);
        println!();
        for announcement in module.get_announcements(&api, false)? {
            println!("=== {} ===", announcement.title);
            let stripped = ammonia::Builder::new().tags(HashSet::new()).clean(&announcement.description).to_string();
            let decoded = htmlescape::decode_html(&stripped).map_err(|_|"Unable to decode HTML Entities")?;
            println!("{}", decoded);
        }
        println!();
        println!();
    }
    Ok(true)
}

fn list_files(api: &Api, modules: &Vec<Module>) -> Result<bool> {
    for module in modules {
        print_files(&module.as_file(&api, true)?, &api, "")?;
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
}
