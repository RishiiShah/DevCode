use clap::Parser;
use devcode::{
    agent::{ExitReason, LoopResult, Role},
    cli::Cli,
    RunOutput,
};
use std::io;

/// Plain REPL slash commands for provider switching (same Session API as the TUI).
fn handle_plain_slash(line: &str, session: &mut devcode::Session) -> bool {
    let mut words = line.split_whitespace();
    let Some(cmd) = words.next() else {
        return false;
    };
    match cmd {
        "/connections" if words.next().is_none() => {
            match session.connections() {
                Ok(lines) => {
                    for line in lines {
                        println!("{}", line);
                    }
                }
                Err(e) => eprintln!("{}", e),
            }
            true
        }
        "/connect" => {
            let name = words.next();
            if words.next().is_some() {
                eprintln!("usage: /connect [NAME]");
            } else if let Err(e) = session.connect(name) {
                eprintln!("{}", e);
            }
            true
        }
        "/egress" => {
            println!(
                "{}",
                session
                    .outbound_preview()
                    .as_deref()
                    .unwrap_or("no model request sent in this session")
            );
            true
        }
        "/status" => {
            println!("{}", session.provider_label());
            if let Some(status) = session.capability_status() {
                println!("{status}");
            }
            if let Some(status) = session.token_status() {
                println!("{status}");
            }
            true
        }
        "/provider" => {
            match words.next() {
                None => {
                    let names = devcode::config::PRESETS
                        .iter()
                        .map(|p| p.name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("providers: {names}");
                    println!("current: {}", session.provider_label());
                }
                Some(name) if words.next().is_none() => match session.set_provider(name) {
                    Ok(Some(warning)) => eprintln!("{warning}"),
                    Ok(None) => println!("{}", session.provider_label()),
                    Err(e) => eprintln!("{e}"),
                },
                _ => eprintln!("usage: /provider [NAME]"),
            }
            true
        }
        "/model" => {
            match words.next() {
                None => match session.list_models() {
                    Ok(models) if models.is_empty() => {
                        println!("current: {}", session.provider_label());
                    }
                    Ok(models) => {
                        for id in models {
                            println!("{id}");
                        }
                        println!("current: {}", session.provider_label());
                    }
                    Err(e) => eprintln!("{e}"),
                },
                Some(id) if words.next().is_none() => match session.set_model(id) {
                    Ok(Some(warning)) => eprintln!("{warning}"),
                    Ok(None) => println!("{}", session.provider_label()),
                    Err(e) => eprintln!("{e}"),
                },
                _ => eprintln!("usage: /model [ID]"),
            }
            true
        }
        "/effort" => {
            match words.next() {
                None => {
                    let levels = session.effort_levels();
                    if levels.is_empty() {
                        println!("reasoning effort is not available for this provider/model");
                    } else {
                        println!("effort levels: {} (or default)", levels.join(", "));
                        println!("current: {}", session.provider_label());
                    }
                }
                Some("default") if words.next().is_none() => match session.set_effort(None) {
                    Ok(Some(warning)) => eprintln!("{warning}"),
                    Ok(None) => println!("{}", session.provider_label()),
                    Err(e) => eprintln!("{e}"),
                },
                Some(level) if words.next().is_none() => match session.set_effort(Some(level)) {
                    Ok(Some(warning)) => eprintln!("{warning}"),
                    Ok(None) => println!("{}", session.provider_label()),
                    Err(e) => eprintln!("{e}"),
                },
                _ => eprintln!("usage: /effort [LEVEL|default]"),
            }
            true
        }
        "/compact" if words.next().is_none() => {
            match session.compact() {
                Ok(msg) => println!("{msg}"),
                Err(e) => eprintln!("{e}"),
            }
            true
        }
        "/export" => {
            let path = line
                .trim()
                .strip_prefix("/export")
                .map(str::trim)
                .unwrap_or("");
            if path.is_empty() {
                eprintln!("usage: /export PATH");
            } else {
                match session.export_markdown(std::path::Path::new(path)) {
                    Ok(dest) => println!("exported conversation to {}", dest.display()),
                    Err(e) => eprintln!("{e}"),
                }
            }
            true
        }
        _ => false,
    }
}

fn print_result(result: &LoopResult) {
    if result.exit_reason == ExitReason::Completed {
        if let Some(message) = result
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && m.tool_calls.is_empty())
        {
            if !message.content.is_empty() {
                println!("{}", message.content);
            }
        }
    }
    if let Some(verification) = &result.verification {
        print!("{}", verification);
    }
    if let Some(summary) = &result.summary {
        println!("{}", summary.format_line());
    }
    if let Some(status) = devcode::agent::token_ledger::summary(&result.token_ledger) {
        println!("{status}");
    }
    if let Some((path, count)) = &result.no_progress {
        eprintln!("no progress: {} repeated {} times", path, count);
    }
    if let Some(reason) = result.exit_reason.budget_message() {
        eprintln!("stopped: {reason}");
    }
    if result.exit_reason == ExitReason::RateLimited {
        eprintln!("rate limited after {} steps", result.steps);
    }
    if result.exit_reason == ExitReason::Cancelled {
        eprintln!("cancelled after {} steps", result.steps);
    }
}
fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                error.exit();
            }
            let args: Vec<_> = std::env::args_os().collect();
            if args.iter().any(|a| a == "auth") && args.iter().any(|a| a == "login") {
                eprintln!("invalid auth login arguments; pass the key via stdin or a TTY");
                std::process::exit(2);
            }
            error.exit();
        }
    };
    if cli.prompt.is_none() && cli.command.is_none() && !cli.plain && devcode::tui::available() {
        if let Err(e) = devcode::tui::run(cli) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
        return;
    }
    let result = devcode::run(cli);
    match result {
        Ok(RunOutput::Rollback(report)) => {
            for path in &report.restored_paths {
                println!("Restored {}", path.display());
            }
            for path in &report.removed_paths {
                println!("Removed {}", path.display());
            }
        }
        Ok(RunOutput::Auth(lines)) => {
            for line in lines {
                println!("{}", line);
            }
        }
        Ok(RunOutput::Gc(report)) => {
            println!(
                "Pruned {} manifest(s) and {} blob(s)",
                report.pruned_manifests, report.pruned_blobs
            );
        }
        Ok(RunOutput::Prompt(result)) => {
            print_result(&result);
            if result.exit_reason.budget_message().is_some() {
                std::process::exit(1);
            }
        }
        Ok(RunOutput::Repl(mut session)) => {
            if let Some(notice) = session.legacy_state_notice() {
                println!("{notice}");
            }
            loop {
                let mut line = String::new();
                match io::stdin().read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) if line.trim() == "/exit" => break,
                    Ok(_) => {
                        if devcode::handle_slash_command(&line, &mut session.profile)
                            || line.trim().is_empty()
                        {
                            continue;
                        }
                        if handle_plain_slash(&line, &mut session) {
                            continue;
                        }
                        match session.prompt(&line) {
                            Ok(result) => print_result(&result),
                            Err(e) => eprintln!("{}", e),
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        break;
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
