use std::fs;

use payee_prompt_individual::{Input, render};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    serde_json::from_str::<Input>(&fs::read_to_string("input.json")?)?;
    fs::write("prompt.md", render()?)?;
    Ok(())
}
