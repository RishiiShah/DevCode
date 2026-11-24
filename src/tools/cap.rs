pub const TOOL_OUTPUT_CAP: usize = 24_000;

/// Truncates string output beyond `cap` bytes, keeping head and tail with a truncation marker.
pub fn truncate_output(input: &str, cap: usize) -> String {
    if input.len() <= cap {
        return input.to_string();
    }

    let omitted = input.len().saturating_sub(cap);
    let marker = format!("\n... [output truncated: {} bytes omitted] ...\n", omitted);

    if marker.len() >= cap {
        return marker.chars().take(cap).collect();
    }

    let budget = cap - marker.len();
    let head_bytes = budget / 2;
    let tail_bytes = budget - head_bytes;

    let head_str = match input
        .char_indices()
        .take_while(|(idx, _)| *idx < head_bytes)
        .last()
    {
        Some((idx, c)) => &input[..idx + c.len_utf8()],
        None => "",
    };

    let tail_start = input.len().saturating_sub(tail_bytes);
    let tail_str = match input.char_indices().find(|(idx, _)| *idx >= tail_start) {
        Some((idx, _)) => &input[idx..],
        None => "",
    };

    let mut out = String::with_capacity(head_str.len() + marker.len() + tail_str.len());
    out.push_str(head_str);
    out.push_str(&marker);
    out.push_str(tail_str);
    out
}

/// Checks if a slice of bytes represents a binary file (contains null bytes).
pub fn is_binary_content(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(8192);
    bytes[..check_len].contains(&0)
}
