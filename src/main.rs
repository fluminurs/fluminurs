mod authorization;

use authorization::Authorization;
use std::io;
use std::io::Write;

fn main() {
    let mut username = String::new();
    let mut password = String::new();
    print!("Username: ");
    io::stdout().flush().unwrap();
    io::stdin().read_line(&mut username);
    print!("Password: ");
    io::stdout().flush().unwrap();
    password = rpassword::read_password().expect("Unable to get non-echo input mode for password");
    username = username.trim().to_string();
    password = password.trim().to_string();
    let mut auth = Authorization::new();
    match auth.login(&username, &password) {
        Ok(_) => println!("{}", auth.jwt.unwrap()),
        Err(error) => println!("{}", error),
    };
}
