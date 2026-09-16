use std::fs;

use monet_server::payee_eval::{EvalInput, Parameters, RenderInvocation, render};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let input = serde_json::from_str::<EvalInput>(&fs::read_to_string("input.json")?)?;
    let rendered = render(RenderInvocation {
        input,
        parameters: Parameters {
            include_potentials: false,
        },
    });
    let prompt = format!(
        "{}\n\n{}\n\nReturn only JSON matching this JSON Schema:\n{}\n",
        rendered.request.system,
        rendered.request.prompt,
        serde_json::to_string(&rendered.request.output_schema)?,
    );
    fs::write("prompt.md", prompt)?;
    Ok(())
}
