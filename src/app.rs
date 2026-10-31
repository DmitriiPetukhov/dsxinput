use crate::controllers;

pub fn run() {
    println!("Hello from the app!");
    if let Err(error) = controllers::detect() {
        eprintln!("Failed to detect controllers: {error}");
    }
}
