mod authorization;

use authorization::Authorization;
use std::io;
use std::io::Write;

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
            println!("JWT: {}", auth.jwt.clone().unwrap());
            match auth.renew() {
                Ok(_) => println!("Renewed: {}", auth.jwt.unwrap()),
                Err(error) => println!("{}", error),
            };
        }
        Err(error) => println!("{}", error),
    };
}
