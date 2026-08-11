use std::io::{self, Read};

use monet_server::payee_eval::{RenderInvocation, render};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    println!(
        "{}",
        serde_json::to_string(&render(serde_json::from_str::<RenderInvocation>(&input)?))?
    );
    Ok(())
}
