mod authorization;

use authorization::Authorization;
use reqwest::Method;
use serde::Deserialize;
use std::io;
use std::io::Write;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Name {
    user_name_original: String,
}

fn main() {
    let mut username = String::new();
    print!("Username: ");
    io::stdout().flush().unwrap();
    io::stdin().read_line(&mut username).expect("Unable to get input");
    username = username.trim().to_string();
    print!("Password: ");
    io::stdout().flush().unwrap();
    let password = rpassword::read_password().expect("Unable to get non-echo input mode for password");
    let mut auth = Authorization::new();
    match auth.login(&username, &password) {
        Ok(_) => {
            let name: Name = auth.api("/user/Profile", Method::GET, None).unwrap().json().unwrap();
            println!("Your name is {}", name.user_name_original);
        }
        Err(error) => println!("{}", error),
    };
}
