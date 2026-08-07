use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use monet_server::categorization::{
    CategorizationFixtures, CategorizationMethod, CategorizationNeighbor, CategorizationPipeline,
    CategorizationQuery, CategorizationResolution, CategoryAccount, CategoryAccountType,
    SourceAccountType,
    run_fixture::{import_transcript, verify_fixture},
};
use serde_json::json;

fn main() {
    let fixtures: CategorizationFixtures =
        serde_json::from_str(include_str!("../categorization/examples.json")).unwrap();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--interactive") {
        run_interactive(&args, &fixtures);
        return;
    }
    let pipeline = CategorizationPipeline::new(&fixtures.accounts, &fixtures.history);
    let results = fixtures
        .evaluations
        .iter()
        .map(|evaluation| (evaluation, pipeline.resolve(&evaluation.query)))
        .collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "accounts": fixtures.accounts,
                "history": fixtures.history,
                "results": results.iter().map(|(evaluation, resolution)| json!({
                    "evaluation": evaluation,
                    "resolution": resolution,
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }

    println!("TRANSACTION CATEGORIZATION EVALUATION");
    println!("=====================================\n");
    print_accounts(&fixtures.accounts);
    println!("\nHISTORICAL EXAMPLES: {}", fixtures.history.len());
    println!("\nEVALUATIONS");
    println!("-----------");
    let mut passed = 0;
    for (evaluation, resolution) in results {
        let pass = resolution.account_id == evaluation.expected_account_id;
        passed += usize::from(pass);
        println!(
            "\n[{}] {}",
            if pass { "PASS" } else { "FAIL" },
            evaluation.name
        );
        println!("  payee:    {}", evaluation.query.payee);
        println!("  amount:   {}", money(evaluation.query.amount));
        println!(
            "  expected: {}",
            evaluation
                .expected_account_id
                .as_deref()
                .unwrap_or("LLM FALLBACK")
        );
        print_resolution(&resolution, "  ");
    }
    println!("\nSUMMARY: {passed}/{} passed", fixtures.evaluations.len());
    if passed != fixtures.evaluations.len() {
        std::process::exit(1);
    }
}

fn run_interactive(args: &[String], fixtures: &CategorizationFixtures) {
    let payees = argument_value(args, "--payee")
        .unwrap_or("Amazon")
        .split(',')
        .map(str::trim)
        .filter(|payee| !payee.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let payees = if payees.is_empty() {
        vec!["Amazon".into()]
    } else {
        payees
    };
    let imported_category = argument_value(args, "--imported-category").map(str::to_owned);
    let fixed_amount = argument_value(args, "--amount").and_then(parse_amount);
    let amount_profile = argument_value(args, "--amount-profile").unwrap_or("default");
    let direction = argument_value(args, "--direction").unwrap_or("outflow");
    let source_type = argument_value(args, "--source-type").unwrap_or("mixed");
    let source_accounts = argument_value(args, "--source-account")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let fixed_date = argument_value(args, "--date").map(str::to_owned);
    let date_profile = argument_value(args, "--date-profile").unwrap_or("recent");
    let seed = argument_value(args, "--seed")
        .and_then(|value| value.parse().ok())
        .unwrap_or(42);
    let steps = argument_value(args, "--steps")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);
    let mut category_history = if args.iter().any(|argument| argument == "--seed-history") {
        fixtures.history.clone()
    } else {
        Vec::new()
    };
    let mut accounts = fixtures.accounts.clone();
    let mut suggestions = 0;
    let mut accepted = 0;
    let mut random = Lcg(seed);
    let mut dates = DateGenerator::new(fixed_date.as_deref(), date_profile);
    let mut session_log = SessionLog::new(args).expect("failed to create categorization log");
    session_log.write(
        "session_start",
        &json!({
            "command": rerun_command(args),
            "payees": &payees,
            "fixedAmount": fixed_amount,
            "amountProfile": amount_profile,
            "direction": direction,
            "sourceType": source_type,
            "sourceAccounts": &source_accounts,
            "fixedDate": &fixed_date,
            "dateProfile": date_profile,
            "steps": steps,
            "seed": seed,
            "seedHistory": args.iter().any(|argument| argument == "--seed-history"),
            "startingExamples": category_history.len(),
            "startingHistory": &category_history,
            "accounts": &accounts,
        }),
    );

    println!("INTERACTIVE TRANSACTION CATEGORIZATION");
    println!("======================================");
    print_accounts(&accounts);
    println!("\nEnter a counterpart account ID for each transaction.");
    println!("An unknown value creates a session-local category account.");
    println!("Press Enter to accept a suggestion, type 'skip', or 'quit'.");
    println!("Session log: {}", session_log.path().display());
    println!(
        "Payees: {}; amount profile: {}; direction: {}; source type: {}; seed: {}\n",
        payees.join(", "),
        fixed_amount.map_or(amount_profile.into(), money),
        direction,
        source_type,
        seed,
    );

    for index in 0..steps {
        let imported_payee = payees[random.next_usize(payees.len())].clone();
        println!("[{}/{}]", index + 1, steps);
        let magnitude = fixed_amount
            .map(i64::unsigned_abs)
            .map(|amount| amount as i64)
            .unwrap_or_else(|| generated_amount(amount_profile, &mut random));
        let amount = match direction {
            "inflow" => magnitude,
            "outflow" => -magnitude,
            _ if fixed_amount.is_some() => fixed_amount.unwrap_or(-magnitude),
            _ if random.next_usize(2) == 0 => -magnitude,
            _ => magnitude,
        };
        let source_account_type = match source_type {
            "asset" => SourceAccountType::Asset,
            "liability" => SourceAccountType::Liability,
            _ if random.next_usize(2) == 0 => SourceAccountType::Asset,
            _ => SourceAccountType::Liability,
        };
        let source_account_id = if source_accounts.is_empty() {
            match source_account_type {
                SourceAccountType::Asset => ["checking", "savings"][random.next_usize(2)].into(),
                SourceAccountType::Liability => {
                    ["credit-card", "loan"][random.next_usize(2)].into()
                }
            }
        } else {
            source_accounts[random.next_usize(source_accounts.len())].clone()
        };
        let query = CategorizationQuery {
            payee: imported_payee,
            imported_category: imported_category.clone(),
            date: dates.next(&mut random),
            amount,
            source_account_type,
            source_account_id,
        };
        let pipeline = CategorizationPipeline::new(&accounts, &category_history);
        let resolution = pipeline.resolve(&query);
        let suggested_category = if resolution.potential_transfer {
            Some("transfer")
        } else {
            resolution.account_id.as_deref()
        };
        session_log.write(
            "step_prompt",
            &json!({
                "step": index + 1,
                "totalSteps": steps,
                "historySize": category_history.len(),
                "query": &query,
                "suggestedAccountId": suggested_category,
                "suggestedAccountName": if resolution.potential_transfer {
                    Some("Potential transfer")
                } else {
                    resolution.account_name.as_deref()
                },
            }),
        );
        println!(
            "Category suggestion: {}",
            if resolution.potential_transfer {
                "POTENTIAL TRANSFER"
            } else {
                resolution
                    .account_name
                    .as_deref()
                    .unwrap_or("UNCATEGORIZED")
            }
        );
        println!("Payee:      {}", query.payee);
        println!("Date:       {}", query.date);
        println!("Amount:     {}", money(query.amount));
        println!(
            "Direction:  {}",
            if query.amount < 0 {
                "outflow"
            } else {
                "inflow"
            }
        );
        println!(
            "Source:     {} ({:?})",
            query.source_account_id, query.source_account_type
        );
        print_resolution(&resolution, "");
        print!("Counterpart account ID or new name> ");
        io::stdout().flush().unwrap();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            session_log.write("session_interrupted", &json!({ "reason": "stdin_eof" }));
            break;
        }
        let raw_answer = answer.trim_end_matches(['\r', '\n']);
        let answer = raw_answer.trim();
        if matches!(answer, "quit" | "q") {
            session_log.write(
                "user_decision",
                &json!({
                    "step": index + 1,
                    "input": raw_answer,
                    "action": "quit",
                    "historySize": category_history.len(),
                }),
            );
            break;
        }
        if answer == "skip" {
            session_log.write(
                "user_decision",
                &json!({
                    "step": index + 1,
                    "input": raw_answer,
                    "action": "skip",
                    "historySize": category_history.len(),
                }),
            );
            println!("Skipped.\n");
            continue;
        }
        let account_id = if answer.is_empty() {
            match &resolution.account_id {
                Some(account_id) => account_id.clone(),
                None => {
                    session_log.write(
                        "user_decision",
                        &json!({
                            "step": index + 1,
                            "input": raw_answer,
                            "action": "no_suggestion",
                            "historySize": category_history.len(),
                        }),
                    );
                    println!("No suggestion to accept.\n");
                    continue;
                }
            }
        } else {
            answer.to_owned()
        };
        let mut created_account = false;
        let account_id = match accounts.iter().find(|account| {
            account.id.eq_ignore_ascii_case(&account_id)
                || account.name.eq_ignore_ascii_case(&account_id)
        }) {
            Some(account) => account.id.clone(),
            None => {
                created_account = true;
                let account_type = if query.amount < 0 {
                    CategoryAccountType::Expense
                } else {
                    CategoryAccountType::Income
                };
                accounts.push(CategoryAccount {
                    id: account_id.clone(),
                    name: account_id.clone(),
                    account_type,
                });
                println!("Created session-local category account: {account_id} ({account_type:?})");
                account_id
            }
        };
        let accepted_suggestion = resolution.account_id.as_ref() == Some(&account_id);
        let decision_type = if accepted_suggestion {
            "accepted_suggestion"
        } else if created_account {
            "proposed_new_category"
        } else if resolution.account_id.is_some() {
            "replaced_suggestion"
        } else {
            "selected_existing_category"
        };
        if let Some(suggested) = &resolution.account_id {
            suggestions += 1;
            if accepted_suggestion {
                accepted += 1;
                println!("Suggestion agreed with your answer.");
            } else {
                println!("Suggestion differed: {suggested} -> {account_id}");
            }
        } else {
            println!("Learned first category example.");
        }
        category_history.push(monet_server::categorization::CategorizationExample {
            payee: query.payee,
            imported_category: query.imported_category,
            date: query.date,
            amount: query.amount,
            source_account_type: query.source_account_type,
            source_account_id: query.source_account_id,
            counterpart_account_id: account_id,
            transfer: false,
        });
        session_log.write(
            "user_decision",
            &json!({
                "step": index + 1,
                "input": raw_answer,
                "action": "categorized",
                "decisionType": decision_type,
                "accountId": category_history.last().map(|example| &example.counterpart_account_id),
                "appliedAccountId": category_history.last().map(|example| &example.counterpart_account_id),
                "suggestedAccountId": suggested_category,
                "createdAccount": created_account,
                "acceptedSuggestion": accepted_suggestion,
                "acceptedProposedCategory": accepted_suggestion,
                "proposedNewCategory": created_account,
                "historySize": category_history.len(),
            }),
        );
        println!(
            "History now contains {} category examples.\n",
            category_history.len()
        );
    }

    println!("SESSION SUMMARY");
    println!("---------------");
    println!("Category examples:    {}", category_history.len());
    println!("Category accounts:    {}", accounts.len());
    println!("Suggestions made:     {suggestions}");
    println!("Suggestions accepted: {accepted}");
    session_log.write(
        "session_end",
        &json!({
            "categoryExamples": category_history.len(),
            "categoryAccounts": accounts.len(),
            "suggestionsMade": suggestions,
            "suggestionsAccepted": accepted,
        }),
    );
    println!("Session log:          {}", session_log.path().display());
    offer_fixture(session_log.path());
}

fn offer_fixture(transcript_path: &Path) {
    print!("Create a regression fixture from this run? [y/N] ");
    io::stdout().flush().unwrap();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).unwrap_or(0) == 0
        || !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    {
        return;
    }
    print!("Fixture name> ");
    io::stdout().flush().unwrap();
    let mut name = String::new();
    if io::stdin().read_line(&mut name).unwrap_or(0) == 0 || name.trim().is_empty() {
        println!("Fixture not created: name is required.");
        return;
    }
    match import_transcript(transcript_path, name.trim()) {
        Ok((fixture, path)) => {
            let failures = verify_fixture(&fixture);
            println!("Created fixture: {}", path.display());
            println!("Rerun: {}", fixture.command);
            if failures.is_empty() {
                println!("Fixture matches the current categorizer.");
            } else {
                println!(
                    "Fixture was created but currently fails:\n{}",
                    failures.join("\n")
                );
            }
        }
        Err(error) => println!("Fixture not created: {error}"),
    }
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
                root.join(".money-tui/logs/categorization-eval")
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
        .expect("failed to write categorization log");
        self.file
            .flush()
            .expect("failed to flush categorization log");
    }
}

fn print_accounts(accounts: &[CategoryAccount]) {
    println!("CATEGORY ACCOUNTS");
    println!("-----------------");
    for account in accounts {
        println!(
            "{}: {} ({:?})",
            account.id, account.name, account.account_type
        );
    }
}

fn print_resolution(resolution: &CategorizationResolution, indent: &str) {
    println!("{indent}method:    {:?}", resolution.method);
    println!("{indent}reason:    {}", resolution.reason);
    println!("{indent}agreement: {:.1}%", resolution.agreement * 100.0);
    println!("{indent}support:   {:.4}", resolution.support);
    println!("{indent}margin:    {:.4}", resolution.margin);
    if !resolution.neighbors.is_empty() {
        println!("{indent}neighbors:");
        for neighbor in &resolution.neighbors {
            print_neighbor(neighbor, indent);
        }
    }
    if resolution.method == CategorizationMethod::LlmFallback {
        println!(
            "{indent}next stage: {}",
            if resolution.neighbors.is_empty() {
                "retained LLM suggestion if available"
            } else {
                "uncategorized; exact history is ambiguous"
            }
        );
    }
}

fn print_neighbor(neighbor: &CategorizationNeighbor, indent: &str) {
    println!(
        "{indent}  {:.4} x {:.4} -> {:.4}  {} -> {}",
        neighbor.similarity, neighbor.recency, neighbor.vote, neighbor.payee, neighbor.account_name
    );
}

fn argument_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn rerun_command(args: &[String]) -> String {
    let mut command = vec![
        "cargo".into(),
        "run".into(),
        "--manifest-path".into(),
        "server/Cargo.toml".into(),
        "--bin".into(),
        "categorization_eval".into(),
        "--".into(),
    ];
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--log" {
            index += 2;
            continue;
        }
        command.push(args[index].clone());
        index += 1;
    }
    command
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_amount(value: &str) -> Option<i64> {
    let parsed = value.parse::<f64>().ok()?;
    Some((parsed * 100.0).round() as i64)
}

fn generated_amount(profile: &str, random: &mut Lcg) -> i64 {
    match profile {
        "small" => 1_000 + random.next_usize(501) as i64,
        "large" => 100_000 + random.next_usize(20_001) as i64,
        "mixed" if random.next_usize(2) == 0 => 1_000 + random.next_usize(501) as i64,
        "mixed" => 100_000 + random.next_usize(20_001) as i64,
        _ => 1_000 + random.next_usize(9_001) as i64,
    }
}

struct DateGenerator {
    day: i64,
    minimum_increment: usize,
    increment_range: usize,
}

impl DateGenerator {
    fn new(start: Option<&str>, profile: &str) -> Self {
        let start = start.unwrap_or(match profile {
            "old" | "mixed" => "2025-01-01",
            _ => "2026-05-01",
        });
        let (minimum_increment, increment_range) = match profile {
            "mixed" => (7, 22),
            _ => (1, 7),
        };
        Self {
            day: civil_days(start).expect("--date must use YYYY-MM-DD"),
            minimum_increment,
            increment_range,
        }
    }

    fn next(&mut self, random: &mut Lcg) -> String {
        let date = civil_date(self.day);
        self.day += (self.minimum_increment + random.next_usize(self.increment_range)) as i64;
        date
    }
}

fn civil_days(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era)
}

fn civil_date(day: i64) -> String {
    let era = if day >= 0 { day } else { day - 146_096 } / 146_097;
    let day_of_era = day - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

struct Lcg(u64);

impl Lcg {
    fn next_usize(&mut self, maximum: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 32) as usize) % maximum
    }
}

fn money(cents: i64) -> String {
    let cents = i128::from(cents);
    let sign = if cents < 0 { "-" } else { "" };
    let absolute = cents.abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_profiles_are_seeded_and_stay_in_their_bands() {
        let mut first = Lcg(42);
        let mut second = Lcg(42);
        let values = (0..20)
            .map(|_| generated_amount("mixed", &mut first))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            (0..20)
                .map(|_| generated_amount("mixed", &mut second))
                .collect::<Vec<_>>()
        );
        assert!(values.iter().all(|amount| {
            (1_000..=1_500).contains(amount) || (100_000..=120_000).contains(amount)
        }));

        let mut default = Lcg(42);
        assert!(
            (0..100).all(|_| (1_000..=10_000).contains(&generated_amount("default", &mut default)))
        );
    }

    #[test]
    fn generated_dates_are_seeded_and_strictly_increasing() {
        let mut random = Lcg(42);
        let mut dates = DateGenerator::new(Some("2025-12-20"), "mixed");
        let generated = (0..30).map(|_| dates.next(&mut random)).collect::<Vec<_>>();

        assert_eq!(generated[0], "2025-12-20");
        assert!(generated.windows(2).all(|dates| dates[0] < dates[1]));
        assert_eq!(civil_date(civil_days("2026-08-07").unwrap()), "2026-08-07");
    }
}
