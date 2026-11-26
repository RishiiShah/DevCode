use crate::guard::path::{canonicalize, is_devcode_path};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CommandCheck {
    Allowed,
    Ask(String),
    Denied(String),
}

/// Strips multiline heredoc bodies from shell scripts prior to lexical analysis.
/// Handles single and multiple heredocs per line (e.g. `cat <<EOF1 <<EOF2`),
/// tab-stripped heredocs (`<<-EOF`), and preserves command text outside heredoc bodies.
pub fn strip_heredocs(script: &str) -> String {
    let lines: Vec<&str> = script.lines().collect();
    if lines.is_empty() {
        return script.to_string();
    }

    let mut kept_lines = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let (cleaned_cmd_line, heredocs) = extract_heredocs_from_line(line);
        kept_lines.push(cleaned_cmd_line);
        i += 1;

        for (delim, strip_tabs) in heredocs {
            while i < lines.len() {
                let cur = lines[i];
                i += 1;
                let test = if strip_tabs {
                    cur.trim_start_matches('\t')
                } else {
                    cur
                };
                if test.trim_end() == delim {
                    break;
                }
            }
        }
    }

    kept_lines.join("\n")
}

fn extract_heredocs_from_line(line: &str) -> (String, Vec<(String, bool)>) {
    let mut cleaned = String::new();
    let mut heredocs = Vec::new();
    let bytes = line.as_bytes();
    let mut idx = 0;
    let mut quote_state: Option<u8> = None;

    while idx < bytes.len() {
        let b = bytes[idx];
        if let Some(q) = quote_state {
            cleaned.push(b as char);
            if b == b'\\' && q == b'"' && idx + 1 < bytes.len() {
                idx += 1;
                cleaned.push(bytes[idx] as char);
            } else if b == q {
                quote_state = None;
            }
            idx += 1;
            continue;
        }

        if b == b'\'' || b == b'"' {
            quote_state = Some(b);
            cleaned.push(b as char);
            idx += 1;
            continue;
        }

        if b == b'#' {
            // Comment: rest of the line is a comment
            cleaned.push_str(&line[idx..]);
            break;
        }

        // Check for heredoc <<
        if b == b'<' && idx + 1 < bytes.len() && bytes[idx + 1] == b'<' {
            if idx + 2 < bytes.len() && bytes[idx + 2] == b'<' {
                // <<< herestring in bash, keep as is
                cleaned.push_str("<<<");
                idx += 3;
                continue;
            }

            idx += 2;
            let mut strip_tabs = false;
            if idx < bytes.len() && bytes[idx] == b'-' {
                strip_tabs = true;
                idx += 1;
            }

            // Skip whitespace
            while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
                idx += 1;
            }

            // Parse delimiter
            let mut delim = String::new();
            if idx < bytes.len() && (bytes[idx] == b'\'' || bytes[idx] == b'"') {
                let delim_quote = bytes[idx];
                idx += 1;
                while idx < bytes.len() && bytes[idx] != delim_quote {
                    delim.push(bytes[idx] as char);
                    idx += 1;
                }
                if idx < bytes.len() {
                    idx += 1;
                }
            } else {
                while idx < bytes.len() {
                    let c = bytes[idx];
                    if c == b' '
                        || c == b'\t'
                        || c == b';'
                        || c == b'|'
                        || c == b'&'
                        || c == b'>'
                        || c == b'<'
                        || c == b')'
                        || c == b'('
                    {
                        break;
                    }
                    delim.push(c as char);
                    idx += 1;
                }
            }

            if !delim.is_empty() {
                heredocs.push((delim, strip_tabs));
                cleaned.push(' ');
            }
            continue;
        }

        cleaned.push(b as char);
        idx += 1;
    }

    (cleaned, heredocs)
}

/// Checks whether `target` resolves into `.devcode/` or a guard-set file.
pub fn is_protected_path(target: &str, root: &Path, guard_set: &[PathBuf]) -> bool {
    let clean_target = target.trim().trim_matches('\'').trim_matches('"');
    if clean_target.is_empty() {
        return false;
    }

    // 1. Check .devcode case-insensitively
    let normalized_slash = clean_target.replace('\\', "/");
    for comp in normalized_slash.split('/') {
        if comp.eq_ignore_ascii_case(".devcode") {
            return true;
        }
    }

    let target_path = Path::new(clean_target);
    if is_devcode_path(root, target_path) || crate::discipline::is_dependency_manifest(target_path)
    {
        return true;
    }

    // 2. Check guard_set
    let rel_target = clean_target.strip_prefix("./").unwrap_or(clean_target);
    let rel_target_path = Path::new(rel_target);

    for g in guard_set {
        let g_rel = g.strip_prefix(root).unwrap_or(g);
        let g_clean = g_rel
            .to_string_lossy()
            .strip_prefix("./")
            .map(PathBuf::from)
            .unwrap_or_else(|| g_rel.to_path_buf());
        if rel_target_path == g_clean || target_path == g || rel_target_path == g {
            return true;
        }
    }

    // Compare canonical paths if possible
    if let Ok(canonical_target) = canonicalize(root, target_path) {
        for g in guard_set {
            if let Ok(cg) = canonicalize(root, g) {
                if canonical_target == cg {
                    return true;
                }
            }
        }
    }

    false
}

/// Performs shell lexical guarding on a command before execution.
pub fn check_shell_command(cmd: &str, root: &Path, guard_set: &[PathBuf]) -> CommandCheck {
    let stripped = strip_heredocs(cmd);
    let lowered = stripped.to_ascii_lowercase();
    let credential_path = crate::credentials::config_dir()
        .ok()
        .map(|dir| dir.join("credentials.json"));
    let configured_path = credential_path
        .as_ref()
        .is_some_and(|path| stripped.contains(&*path.to_string_lossy()));
    if lowered.contains("devcode/credentials")
        || stripped.contains("DEVCODE_CONFIG_DIR")
        || configured_path
    {
        return CommandCheck::Denied("access to stored credentials is denied".into());
    }
    let subcommands = split_subcommands(&stripped);

    for subcmd in subcommands {
        let trimmed = subcmd.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Tokenize subcommand into words and inspect redirections
        let parsed = match parse_subcommand(trimmed, root, guard_set) {
            Ok(p) => p,
            Err(denied) => return CommandCheck::Denied(denied),
        };

        if parsed.words.is_empty() {
            continue;
        }

        let cmd_token = &parsed.words[0];
        let bin_name = Path::new(cmd_token)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd_token);

        // Check utilities targeting protected paths: tee, cp, mv
        match bin_name {
            "tee" => {
                for arg in parsed.words.iter().skip(1) {
                    if !arg.starts_with('-') && is_protected_path(arg, root, guard_set) {
                        return CommandCheck::Denied(format!(
                            "'tee' targeting protected path '{}' denied. Use file tools instead.",
                            arg
                        ));
                    }
                }
            }
            "cp" => {
                if let Some(dest) = parsed.words.iter().skip(1).rfind(|w| !w.starts_with('-')) {
                    if is_protected_path(dest, root, guard_set) {
                        return CommandCheck::Denied(format!(
                            "'cp' targeting protected path '{}' denied. Use file tools instead.",
                            dest
                        ));
                    }
                }
            }
            "mv" => {
                if let Some(dest) = parsed.words.iter().skip(1).rfind(|w| !w.starts_with('-')) {
                    if is_protected_path(dest, root, guard_set) {
                        return CommandCheck::Denied(format!(
                            "'mv' targeting protected path '{}' denied. Use file tools instead.",
                            dest
                        ));
                    }
                }
                // If not targeting protected path, mv is still a destructive pattern
                return CommandCheck::Ask("mv: destructive command pattern".to_string());
            }
            "rm" => {
                // Deny `rm` of `.devcode/` (legacy/fallback state) and broad dotfile globs.
                for arg in parsed.words.iter().skip(1).filter(|w| !w.starts_with('-')) {
                    let bare = arg.trim_matches('\'').trim_matches('"');
                    let wipes_dotfiles = matches!(
                        bare.trim_end_matches('/'),
                        "." | ".." | ".*" | ".??*" | ".[!.]*" | "./.*"
                    );
                    let in_devcode = bare
                        .replace('\\', "/")
                        .split('/')
                        .any(|c| c.eq_ignore_ascii_case(".devcode"));
                    if wipes_dotfiles || in_devcode {
                        return CommandCheck::Denied(format!(
                            "'rm {bare}' would remove DevCode's protected .devcode/ state or dotfiles; name the files to remove instead"
                        ));
                    }
                }
                return CommandCheck::Ask("rm: destructive command pattern".to_string());
            }
            "sed" => {
                if parsed.words.iter().any(|w| {
                    w == "-i"
                        || (w.starts_with("-i") && w.len() > 2)
                        || w == "--in-place"
                        || w.starts_with("--in-place=")
                }) {
                    return CommandCheck::Ask(
                        "sed -i: destructive in-place modification".to_string(),
                    );
                }
            }
            "git" => {
                let subcmd_opt = parsed
                    .words
                    .iter()
                    .skip(1)
                    .find(|w| !w.starts_with('-'))
                    .map(|s| s.as_str());
                match subcmd_opt {
                    Some("clean") => {
                        return CommandCheck::Ask(
                            "git clean: destructive command pattern".to_string(),
                        );
                    }
                    Some("restore") => {
                        return CommandCheck::Ask(
                            "git restore: destructive command pattern".to_string(),
                        );
                    }
                    Some("stash") => {
                        return CommandCheck::Ask(
                            "git stash: destructive command pattern".to_string(),
                        );
                    }
                    Some("reset") if parsed.words.iter().any(|w| w == "--hard") => {
                        return CommandCheck::Ask(
                            "git reset --hard: destructive command pattern".to_string(),
                        );
                    }
                    Some("checkout") if parsed.words.iter().any(|w| w == "--") => {
                        return CommandCheck::Ask(
                            "git checkout --: destructive command pattern".to_string(),
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    CommandCheck::Allowed
}

struct ParsedSubcommand {
    words: Vec<String>,
}

fn parse_subcommand(
    subcmd: &str,
    root: &Path,
    guard_set: &[PathBuf],
) -> Result<ParsedSubcommand, String> {
    let bytes = subcmd.as_bytes();
    let mut idx = 0;
    let mut words = Vec::new();
    let mut current_word = String::new();

    while idx < bytes.len() {
        let b = bytes[idx];

        if b == b' ' || b == b'\t' {
            if !current_word.is_empty() {
                words.push(current_word.clone());
                current_word.clear();
            }
            idx += 1;
            continue;
        }

        if b == b'\'' || b == b'"' {
            let quote = b;
            idx += 1;
            while idx < bytes.len() {
                let qb = bytes[idx];
                if qb == b'\\' && quote == b'"' && idx + 1 < bytes.len() {
                    idx += 1;
                    current_word.push(bytes[idx] as char);
                } else if qb == quote {
                    idx += 1;
                    break;
                } else {
                    current_word.push(qb as char);
                }
                idx += 1;
            }
            continue;
        }

        // Check for redirection operators: >, >>, &>, >|, 1>, 2>, etc.
        let is_digit_fd = current_word.chars().all(|c| c.is_ascii_digit());
        let is_amp_redirection = b == b'&' && idx + 1 < bytes.len() && bytes[idx + 1] == b'>';
        let is_gt_redirection = b == b'>';

        if (is_amp_redirection || is_gt_redirection) && (current_word.is_empty() || is_digit_fd) {
            current_word.clear();

            if is_amp_redirection {
                idx += 2;
            } else {
                idx += 1;
                if idx < bytes.len() && (bytes[idx] == b'>' || bytes[idx] == b'|') {
                    idx += 1;
                }
            }

            // Skip whitespace after redirection operator
            while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
                idx += 1;
            }

            // Parse target token
            let mut target = String::new();
            if idx < bytes.len() && (bytes[idx] == b'\'' || bytes[idx] == b'"') {
                let quote = bytes[idx];
                idx += 1;
                while idx < bytes.len() && bytes[idx] != quote {
                    target.push(bytes[idx] as char);
                    idx += 1;
                }
                if idx < bytes.len() {
                    idx += 1;
                }
            } else {
                while idx < bytes.len() {
                    let tb = bytes[idx];
                    if tb == b' '
                        || tb == b'\t'
                        || tb == b';'
                        || tb == b'|'
                        || tb == b'&'
                        || tb == b'>'
                        || tb == b'<'
                    {
                        break;
                    }
                    target.push(tb as char);
                    idx += 1;
                }
            }

            // Check if target is a file descriptor duplication (e.g. &1, &2, &-)
            let is_fd_dup = target.starts_with('&')
                && target[1..].chars().all(|c| c.is_ascii_digit() || c == '-');

            if !is_fd_dup && is_protected_path(&target, root, guard_set) {
                return Err(format!(
                    "Redirection to protected path '{}' denied. Use file tools instead.",
                    target
                ));
            }

            continue;
        }

        current_word.push(b as char);
        idx += 1;
    }

    if !current_word.is_empty() {
        words.push(current_word);
    }

    Ok(ParsedSubcommand { words })
}

fn split_subcommands(script: &str) -> Vec<String> {
    let mut subcmds = Vec::new();
    let mut current = String::new();
    let bytes = script.as_bytes();
    let mut idx = 0;
    let mut quote_state: Option<u8> = None;

    while idx < bytes.len() {
        let b = bytes[idx];

        if let Some(q) = quote_state {
            current.push(b as char);
            if b == b'\\' && q == b'"' && idx + 1 < bytes.len() {
                idx += 1;
                current.push(bytes[idx] as char);
            } else if b == q {
                quote_state = None;
            }
            idx += 1;
            continue;
        }

        if b == b'\'' || b == b'"' {
            quote_state = Some(b);
            current.push(b as char);
            idx += 1;
            continue;
        }

        if b == b';' || b == b'\n' {
            if !current.trim().is_empty() {
                subcmds.push(current.clone());
            }
            current.clear();
            idx += 1;
            continue;
        }

        if b == b'&' && idx + 1 < bytes.len() && bytes[idx + 1] == b'&' {
            if !current.trim().is_empty() {
                subcmds.push(current.clone());
            }
            current.clear();
            idx += 2;
            continue;
        }

        if b == b'|' && idx + 1 < bytes.len() && bytes[idx + 1] == b'|' {
            if !current.trim().is_empty() {
                subcmds.push(current.clone());
            }
            current.clear();
            idx += 2;
            continue;
        }

        if b == b'|' {
            if current.ends_with('>') {
                // >| clobber redirection, do not split
                current.push(b as char);
                idx += 1;
                continue;
            }
            if !current.trim().is_empty() {
                subcmds.push(current.clone());
            }
            current.clear();
            idx += 1;
            continue;
        }

        // Single background & (not &> and not preceded by >)
        if b == b'&'
            && (idx + 1 >= bytes.len() || bytes[idx + 1] != b'>')
            && !current.ends_with('>')
            && !current.trim().is_empty()
        {
            subcmds.push(current.clone());
            current.clear();
            idx += 1;
            continue;
        }

        current.push(b as char);
        idx += 1;
    }

    if !current.trim().is_empty() {
        subcmds.push(current);
    }

    subcmds
}
