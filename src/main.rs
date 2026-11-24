use clap::Parser;
use devcode::{
    agent::{ExitReason, LoopResult, Role},
    cli::Cli,
    RunOutput,
};
use std::io;

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
    if let Some((path, count)) = &result.no_progress {
        eprintln!("no progress: {} repeated {} times", path, count);
    }
    if result.exit_reason == ExitReason::RateLimited {
        eprintln!("rate limited after {} steps", result.steps);
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
        Ok(RunOutput::Prompt(result)) => print_result(&result),
        Ok(RunOutput::Repl(mut session)) => loop {
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
                    let mut words = line.split_whitespace();
                    match words.next() {
                        Some("/connections") if words.next().is_none() => {
                            match session.connections() {
                                Ok(lines) => {
                                    for line in lines {
                                        println!("{}", line);
                                    }
                                }
                                Err(e) => eprintln!("{}", e),
                            }
                            continue;
                        }
                        Some("/connect") => {
                            let name = words.next();
                            if words.next().is_some() {
                                eprintln!("usage: /connect [NAME]");
                                continue;
                            }
                            if let Err(e) = session.connect(name) {
                                eprintln!("{}", e);
                            }
                            continue;
                        }
                        _ => {}
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
        },
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
