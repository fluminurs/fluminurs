use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::path::Path;

use clap::{App, Arg};
use futures::{future, Future};
use serde::{Deserialize, Serialize};
use tokio;

use crate::api::Api;
use crate::api::module::{File, Module};
use futures::future::Either;

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

fn print_swallow<T: std::fmt::Display>(value: T) {
    println!("{}", value);
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

fn print_announcements(api: &Api, modules: &[Module])
    -> impl Future<Item=(), Error=Error> + 'static {
    let apic = api.clone();
    let futures = modules.into_iter()
        .map( |module| {
            let module_code = module.code.clone();
            let module_name = module.name.clone();
            module.get_announcements(&apic, false)
                .map(move |anns| (module_code, module_name, anns))
        })
        .collect::<Vec<_>>();
    future::join_all(futures)
        .and_then(|mods_anns| {
            for (module_code, module_name, anns) in mods_anns {
                println!("# {} {}", module_code, module_name);
                println!();
                for ann in anns {
                    println!("=== {} ===", ann.title);
                    let stripped = ammonia::Builder::new()
                        .tags(HashSet::new())
                        .clean(&ann.description)
                        .to_string();
                    let decoded =
                        htmlescape::decode_html(&stripped)
                            .unwrap_or_else(|_| "Unable to decode HTML Entities".to_owned());
                    println!("{}", decoded);
                }
                println!();
                println!();
            }
            future::result(Ok(()))
        })
}

fn load_modules_files(api: &Api, modules: &[Module])
    -> impl Future<Item=Vec<File>, Error=Error> + 'static {
    let apic = api.clone();
    let files = modules.iter().map(Module::as_file).collect::<Vec<File>>();
    future::join_all(files.into_iter()
        .map(move |f| f.load_all_children(&apic).map(|_| f)))
}

fn list_files(api: &Api, modules: &[Module])
    -> impl Future<Item=(), Error=Error> + 'static {
    load_modules_files(api, modules)
        .and_then(|files| {
            for file in files {
                print_files(&file, "");
            }
            future::result(Ok(()))
        })
}

fn download_file(api: &Api, file: &File, path: &Path) {
    let path = path.to_path_buf();
    tokio::spawn(file.download(api.clone(), &path)
        .map(move |result| if result {
            println!("Downloaded to {}", path.to_string_lossy());
        })
        .map_err(|e| {
            println!("Failed to download file: {}", e);
        }));
}

fn download_files(api: &Api, modules: &[Module], destination: &str) -> Result<()> {
    fn recurse_files(api: &Api, file: File, path: &Path) {
        let path = path.join(file.name());
        if file.is_directory() {
            for child in file.children().expect("Children should have been loaded") {
                recurse_files(api, child, &path);
            }
        } else {
            download_file(api, &file, &path);
        }
    }

    println!("Download to {}", destination);
    let path = Path::new(destination).to_owned();
    if !path.is_dir() {
        return Err("Download destination does not exist or is not a directory");
    }
    for module in modules {
        let file = module.as_file();
        let api_clone = api.clone();
        let path_clone = path.clone();
        tokio::spawn(file.load_all_children(api)
            .map_err(|e| {
                println!("Failed to load children: {}", e);
            })
            .and_then(move |_| {
                recurse_files(&api_clone, file, &path_clone);
                future::result(Ok(()))
            }));
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
        .arg(
            Arg::with_name("credential-file")
                .long("credential-file")
                .takes_value(true),
        )
        .get_matches();
    let credential_file = matches.value_of("credential-file").unwrap_or("login.json")
        .to_owned();
    let do_announcements = matches.is_present("announcements");
    let do_files = matches.is_present("files");
    let download_destination = matches.value_of("download").map(|s| s.to_owned());

    let (username, password) = get_credentials(&credential_file).expect("Unable to get credentials");
    tokio::run(Api::with_login(&username, &password)
        .and_then(move |api| future::result(if !Path::new(&credential_file).exists() {
                store_credentials(&credential_file, &username, &password)
            } else {
                Ok(())
            })
            .and_then(move |_| api.name().map(|r| (api, r))))
        .and_then(|(api, name)| {
            println!("Hi {}!", name);
            api.modules(true).map(|r| (api, r))
        })
        .and_then(move |(api, modules)| {
            println!("You are taking:");
            for module in modules.iter().filter(|m| m.is_taking()) {
                println!("- {} {}", module.code, module.name);
            }
            println!("You are teaching:");
            for module in modules.iter().filter(|m| m.is_teaching()) {
                println!("- {} {}", module.code, module.name);
            }

            if do_announcements {
                Either::A(print_announcements(&api, &modules))
            } else {
                Either::B(future::result(Ok(())))
            }.join(if do_files {
                Either::A(list_files(&api, &modules))
            } else {
                Either::B(future::result(Ok(())))
            }).join(future::result(if let Some(destination) = download_destination {
                download_files(&api, &modules, &destination)
            } else {
                Ok(())
            }))
                .map(|_| ())
        })
        .map_err(print_swallow))
}
