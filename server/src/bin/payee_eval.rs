use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use monet_server::payee_resolution::{
    PayeeCandidate, PayeeExample, PayeeFixtures, PayeeQuery, PayeeResolution, PayeeResolver,
    generate_fuzzy_payees, generate_related_fuzzy_payees, normalize_payee,
};
use serde_json::json;

fn main() {
    let fixtures: PayeeFixtures =
        serde_json::from_str(include_str!("../payee_resolution/examples.json")).unwrap();
    let resolver = PayeeResolver::new(&fixtures.history);
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let json_output = args.iter().any(|argument| argument == "--json");
    if args.iter().any(|argument| argument == "--interactive") {
        run_interactive(&args, &fixtures);
        return;
    }
    let query = args
        .iter()
        .find(|argument| !argument.starts_with("--"))
        .cloned();

    if let Some(imported_payee) = query {
        let resolution = resolver.resolve(&PayeeQuery { imported_payee });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&resolution).unwrap());
        } else {
            print_resolution(&resolution, "");
        }
        return;
    }

    let results = fixtures
        .evaluations
        .iter()
        .map(|evaluation| {
            let resolution = resolver.resolve(&PayeeQuery {
                imported_payee: evaluation.imported_payee.clone(),
            });
            (evaluation, resolution)
        })
        .collect::<Vec<_>>();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "groups": resolver.groups(),
                "results": results.iter().map(|(evaluation, resolution)| json!({
                    "evaluation": evaluation,
                    "resolution": resolution,
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }

    println!("PAYEE RESOLUTION EVALUATION");
    println!("===========================\n");
    println!("HISTORICAL RESOLVED PAYEES");
    println!("--------------------------");
    for group in resolver.groups() {
        println!("{} ({} examples)", group.resolved_payee, group.examples);
        for alias in group.aliases {
            println!("  <- {alias}");
        }
    }

    println!("\nEVALUATIONS");
    println!("-----------");
    let mut passed = 0;
    for (evaluation, resolution) in &results {
        let pass = resolution.resolved_payee == evaluation.expected;
        passed += usize::from(pass);
        println!(
            "\n[{}] {}",
            if pass { "PASS" } else { "FAIL" },
            evaluation.name
        );
        println!(
            "  expected: {}",
            evaluation.expected.as_deref().unwrap_or("UNRESOLVED")
        );
        print_resolution(resolution, "  ");
    }
    println!("\nSUMMARY: {passed}/{} passed", results.len());
    if passed != results.len() {
        std::process::exit(1);
    }
}

fn run_interactive(args: &[String], fixtures: &PayeeFixtures) {
    let steps = argument_value(args, "--steps")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);
    let seed = argument_value(args, "--seed")
        .and_then(|value| value.parse().ok())
        .unwrap_or(42);
    let payee = argument_value(args, "--payee").unwrap_or("Starbucks");
    let related = args.iter().any(|argument| argument == "--related");
    let mut history = if args.iter().any(|argument| argument == "--seed-history") {
        fixtures.history.clone()
    } else {
        Vec::new()
    };
    let generated = if related {
        generate_related_fuzzy_payees(payee, seed, steps)
    } else {
        generate_fuzzy_payees(payee, seed, steps)
    };
    let mut suggested = 0;
    let mut agreed = 0;
    let mut session_log = SessionLog::new(args).expect("failed to create payee evaluation log");
    session_log.write(
        "session_start",
        &json!({
            "payee": payee,
            "related": related,
            "steps": steps,
            "seed": seed,
            "seedHistory": args.iter().any(|argument| argument == "--seed-history"),
            "startingExamples": history.len(),
        }),
    );

    println!("INTERACTIVE PAYEE LEARNING");
    println!("==========================");
    println!("Enter the resolved payee you want for each imported value.");
    println!(
        "Press Enter to accept a suggestion, type 'skip' to add no example, or 'quit' to stop."
    );
    println!(
        "Generated payee{}: {payee}",
        if related { " family" } else { "" }
    );
    println!("Starting with {} historical examples.\n", history.len());
    println!("Session log: {}\n", session_log.path().display());

    for (index, generated) in generated.into_iter().enumerate() {
        let resolver = PayeeResolver::new(&history);
        let resolution = resolver.resolve(&PayeeQuery {
            imported_payee: generated.imported_payee.clone(),
        });
        session_log.write(
            "step_prompt",
            &json!({
                "step": index + 1,
                "totalSteps": steps,
                "importedPayee": &generated.imported_payee,
                "generatedCanonicalPayee": &generated.canonical_payee,
                "historySize": history.len(),
                "resolution": &resolution,
            }),
        );
        println!("[{}/{}]", index + 1, steps);
        println!("Imported:   {}", generated.imported_payee);
        println!(
            "Suggestion: {}",
            resolution.resolved_payee.as_deref().unwrap_or("UNRESOLVED")
        );
        print_decision(&resolution, "");
        println!("Reason:     {}", resolution.reason);
        print_candidates(&resolution.candidates, "");
        print!("Resolved payee> ");
        io::stdout().flush().unwrap();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            session_log.write("session_interrupted", &json!({ "reason": "stdin_eof" }));
            break;
        }
        let raw_answer = answer.trim_end_matches(['\r', '\n']);
        let answer = raw_answer.trim();
        if answer.eq_ignore_ascii_case("quit") || answer.eq_ignore_ascii_case("q") {
            session_log.write(
                "user_decision",
                &json!({
                    "step": index + 1,
                    "input": raw_answer,
                    "action": "quit",
                    "historySize": history.len(),
                }),
            );
            break;
        }
        if answer.eq_ignore_ascii_case("skip") {
            session_log.write(
                "user_decision",
                &json!({
                    "step": index + 1,
                    "input": raw_answer,
                    "action": "skip",
                    "historySize": history.len(),
                }),
            );
            println!("Skipped; resolver unchanged.\n");
            continue;
        }
        let resolved = if answer.is_empty() {
            match &resolution.resolved_payee {
                Some(suggestion) => suggestion.clone(),
                None => {
                    session_log.write(
                        "user_decision",
                        &json!({
                            "step": index + 1,
                            "input": raw_answer,
                            "action": "no_suggestion",
                            "historySize": history.len(),
                        }),
                    );
                    println!("No suggestion to accept; resolver unchanged.\n");
                    continue;
                }
            }
        } else {
            answer.to_owned()
        };
        if let Some(suggestion) = &resolution.resolved_payee {
            suggested += 1;
            if normalize_payee(suggestion) == normalize_payee(&resolved) {
                agreed += 1;
                println!("Suggestion agreed with your answer.");
            } else {
                println!("Suggestion differed: {suggestion} -> {resolved}");
            }
        } else {
            println!("Learned first example for {resolved}.");
        }
        history.push(PayeeExample {
            imported_payee: generated.imported_payee,
            resolved_payee: resolved.clone(),
        });
        session_log.write(
            "user_decision",
            &json!({
                "step": index + 1,
                "input": raw_answer,
                "action": "resolved",
                "resolvedPayee": &resolved,
                "acceptedSuggestion": resolution.resolved_payee.as_ref().is_some_and(|suggestion| {
                    normalize_payee(suggestion) == normalize_payee(&resolved)
                }),
                "historySize": history.len(),
            }),
        );
        println!("History now contains {} examples.\n", history.len());
    }

    println!("SESSION SUMMARY");
    println!("---------------");
    println!("Historical examples: {}", history.len());
    println!("Suggestions made:     {suggested}");
    println!("Suggestions accepted: {agreed}");
    if suggested > 0 {
        println!(
            "Agreement:             {:.1}%",
            agreed as f64 / suggested as f64 * 100.0
        );
    }
    println!(
        "Resolved groups:       {}",
        PayeeResolver::new(&history).groups().len()
    );
    session_log.write(
        "session_end",
        &json!({
            "historicalExamples": history.len(),
            "suggestionsMade": suggested,
            "suggestionsAccepted": agreed,
            "resolvedGroups": PayeeResolver::new(&history).groups().len(),
        }),
    );
    println!("Session log:           {}", session_log.path().display());
}

struct SessionLog {
    path: PathBuf,
    file: File,
}

impl SessionLog {
    fn new(args: &[String]) -> io::Result<Self> {
        let path = match argument_value(args, "--log") {
            Some(path) => PathBuf::from(path),
            None => {
                let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
                let root = manifest.parent().unwrap_or(manifest);
                root.join(".money-tui/logs/payee-eval")
                    .join(format!("{}.jsonl", uuid::Uuid::now_v7()))
            }
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&mut self, event: &str, data: &serde_json::Value) {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        writeln!(
            self.file,
            "{}",
            json!({
                "timestampMs": timestamp_ms,
                "event": event,
                "data": data,
            })
        )
        .expect("failed to write payee evaluation log");
        self.file
            .flush()
            .expect("failed to flush payee evaluation log");
    }
}

fn argument_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn print_resolution(resolution: &PayeeResolution, indent: &str) {
    println!("{indent}imported: {}", resolution.imported_payee);
    println!(
        "{indent}resolved: {}",
        resolution.resolved_payee.as_deref().unwrap_or("UNRESOLVED")
    );
    print_decision(resolution, indent);
    println!("{indent}reason:   {}", resolution.reason);
    print_candidates(&resolution.candidates, indent);
}

fn print_decision(resolution: &PayeeResolution, indent: &str) {
    println!("{indent}method:                {:?}", resolution.method);
    println!(
        "{indent}best fuzzy score:      {:.4}",
        resolution.best_score
    );
    println!(
        "{indent}minimum fuzzy score:   {:.4}",
        resolution.minimum_score
    );
    println!(
        "{indent}runner-up group score: {}",
        resolution
            .runner_up_score
            .map_or_else(|| "none".into(), |score| format!("{score:.4}"))
    );
    println!(
        "{indent}minimum group margin:  {:.4}",
        resolution.minimum_group_margin
    );
    println!(
        "{indent}selected history pos:  {}",
        resolution
            .selected_position
            .map_or_else(|| "none".into(), |position| position.to_string())
    );
}

fn print_candidates(candidates: &[PayeeCandidate], indent: &str) {
    if candidates.is_empty() {
        println!("{indent}candidates: none");
        return;
    }
    println!("{indent}candidates:");
    for candidate in candidates.iter().take(10) {
        println!(
            "{indent}  {}{}: {:.4} at position {} -> {}",
            if candidate.selected { "SELECTED " } else { "" },
            if candidate.eligible {
                "ELIGIBLE"
            } else {
                "INELIGIBLE"
            },
            candidate.score,
            candidate.position,
            candidate.resolved_payee,
        );
    }
}
