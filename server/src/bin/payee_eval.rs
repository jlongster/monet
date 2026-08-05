use std::io::{self, Write};

use monet_server::payee_resolution::{
    PayeeCandidate, PayeeExample, PayeeFixtures, PayeeQuery, PayeeResolution, PayeeResolver,
    generate_fuzzy_payees, normalize_payee,
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
    let mut history = if args.iter().any(|argument| argument == "--seed-history") {
        fixtures.history.clone()
    } else {
        Vec::new()
    };
    let generated = generate_fuzzy_payees(payee, seed, steps);
    let mut suggested = 0;
    let mut agreed = 0;

    println!("INTERACTIVE PAYEE LEARNING");
    println!("==========================");
    println!("Enter the resolved payee you want for each imported value.");
    println!(
        "Press Enter to accept a suggestion, type 'skip' to add no example, or 'quit' to stop."
    );
    println!("Generated payee: {payee}");
    println!("Starting with {} historical examples.\n", history.len());

    for (index, generated) in generated.into_iter().enumerate() {
        let resolver = PayeeResolver::new(&history);
        let resolution = resolver.resolve(&PayeeQuery {
            imported_payee: generated.imported_payee.clone(),
        });
        println!("[{}/{}]", index + 1, steps);
        println!("Imported:   {}", generated.imported_payee);
        println!(
            "Suggestion: {}",
            resolution.resolved_payee.as_deref().unwrap_or("UNRESOLVED")
        );
        print_decision(&resolution, "");
        println!("Reason:     {}", resolution.reason);
        print_neighbors(&resolution, "");
        print_candidates(&resolution.candidates, "");
        print!("Resolved payee> ");
        io::stdout().flush().unwrap();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            break;
        }
        let answer = answer.trim();
        if answer.eq_ignore_ascii_case("quit") || answer.eq_ignore_ascii_case("q") {
            break;
        }
        if answer.eq_ignore_ascii_case("skip") {
            println!("Skipped; resolver unchanged.\n");
            continue;
        }
        let resolved = if answer.is_empty() {
            match &resolution.resolved_payee {
                Some(suggestion) => suggestion.clone(),
                None => {
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
            resolved_payee: resolved,
        });
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
    print_neighbors(resolution, indent);
    print_candidates(&resolution.candidates, indent);
}

fn print_decision(resolution: &PayeeResolution, indent: &str) {
    println!("{indent}method:      {:?}", resolution.method);
    if let Some(vote_leader) = resolution.candidates.first() {
        println!(
            "{indent}vote leader:  {} (vote {:.4}, {} neighbors, {:.1}% agreement)",
            vote_leader.resolved_payee,
            vote_leader.neighbor_vote,
            vote_leader.neighbor_count,
            vote_leader.neighbor_agreement * 100.0
        );
        if let Some(lexical_leader) = resolution
            .candidates
            .iter()
            .max_by(|left, right| left.score.total_cmp(&right.score))
        {
            println!(
                "{indent}best lexical: {} ({:.4})",
                lexical_leader.resolved_payee, lexical_leader.score
            );
        }
    } else {
        println!("{indent}vote leader:  none");
        println!("{indent}best lexical: none");
    }
    println!("{indent}vote margin:  {:.4}", resolution.margin);
    println!("{indent}support:      {:.4}", resolution.effective_support);
}

fn print_neighbors(resolution: &PayeeResolution, indent: &str) {
    if resolution.neighbors.is_empty() {
        return;
    }
    println!("{indent}nearest examples:");
    for neighbor in &resolution.neighbors {
        println!(
            "{indent}  {:.4} x {:.4} -> {:.4}  {} -> {}",
            neighbor.similarity,
            neighbor.recency,
            neighbor.vote,
            neighbor.imported_payee,
            neighbor.resolved_payee
        );
    }
}

fn print_candidates(candidates: &[PayeeCandidate], indent: &str) {
    if candidates.is_empty() {
        println!("{indent}candidates: none");
        return;
    }
    println!("{indent}candidates:");
    for candidate in candidates.iter().take(3) {
        println!(
            "{indent}  {}: lexical {:.4}; vote {:.4}; neighbors {}; agreement {:.1}% (char {:.4}, tfidf {:.4}, token {:.4})",
            candidate.resolved_payee,
            candidate.score,
            candidate.neighbor_vote,
            candidate.neighbor_count,
            candidate.neighbor_agreement * 100.0,
            candidate.character_score,
            candidate.tfidf_score,
            candidate.token_score,
        );
    }
}
