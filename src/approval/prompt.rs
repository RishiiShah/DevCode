use crate::approval::diff::format_diff;
use std::io::{BufRead, IsTerminal, Write};

/// Manages interactive and automated approvals for file edits and destructive commands.
pub struct ApprovalPrompt<R, W> {
    pub reader: R,
    pub writer: W,
    pub is_tty: bool,
    pub auto_approve_edits: bool,
    pub always_approved: bool,
}

impl<R: BufRead, W: Write> ApprovalPrompt<R, W> {
    pub fn new(reader: R, writer: W, is_tty: bool, auto_approve_edits: bool) -> Self {
        Self {
            reader,
            writer,
            is_tty,
            auto_approve_edits,
            always_approved: false,
        }
    }

    /// Requests user approval for a proposed file edit.
    /// In non-interactive mode (no TTY), edits are rejected unless `--yes` was provided.
    pub fn request_edit(
        &mut self,
        path: &str,
        old_content: &str,
        new_content: &str,
    ) -> Result<bool, std::io::Error> {
        if self.always_approved || self.auto_approve_edits {
            return Ok(true);
        }

        if !self.is_tty {
            writeln!(
                self.writer,
                "Edits rejected: no TTY and --yes not specified"
            )?;
            return Ok(false);
        }

        let diff = format_diff(old_content, new_content, path);
        write!(self.writer, "{}", diff)?;
        write!(
            self.writer,
            "Approve edit to '{}'? [y]es / [a]lways / [n]o: ",
            path
        )?;
        self.writer.flush()?;

        let mut input = String::new();
        if self.reader.read_line(&mut input)? == 0 {
            return Ok(false);
        }

        let trimmed = input.trim().to_lowercase();
        match trimmed.as_str() {
            "y" | "yes" | "once" | "o" => Ok(true),
            "a" | "always" => {
                self.always_approved = true;
                Ok(true)
            }
            "n" | "no" | "reject" | "r" => Ok(false),
            _ => Ok(false),
        }
    }

    /// Requests user approval for a destructive command pattern.
    /// Note: Destructive patterns always ask, even when `--yes` is active.
    /// With no TTY, destructive commands are strictly denied.
    pub fn request_command(&mut self, command: &str, reason: &str) -> Result<bool, std::io::Error> {
        if !self.is_tty {
            writeln!(self.writer, "Destructive command rejected: no TTY")?;
            return Ok(false);
        }

        write!(
            self.writer,
            "Destructive command detected: '{}' ({}). Approve? [y]es / [n]o: ",
            command, reason
        )?;
        self.writer.flush()?;

        let mut input = String::new();
        if self.reader.read_line(&mut input)? == 0 {
            return Ok(false);
        }

        let trimmed = input.trim().to_lowercase();
        match trimmed.as_str() {
            "y" | "yes" | "once" | "o" => Ok(true),
            _ => Ok(false),
        }
    }
}

/// Creates a standard terminal-attached approval prompt.
pub fn terminal_prompt(
    auto_approve_edits: bool,
) -> ApprovalPrompt<std::io::BufReader<std::io::Stdin>, std::io::Stderr> {
    let stdin = std::io::stdin();
    let is_tty = stdin.is_terminal();
    ApprovalPrompt::new(
        std::io::BufReader::new(stdin),
        std::io::stderr(),
        is_tty,
        auto_approve_edits,
    )
}
