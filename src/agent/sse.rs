//! Server-sent events and chunk-safe secret redaction for model streams.
//!
//! `Model::redact_sensitive` replaces the provider API key in a finished string.
//! That replace is not safe on arbitrary chunks, because the key can be split
//! across them. [`RedactTail`] is that same replacement done incrementally:
//! a finished key is rewritten before it is emitted, and concatenating every
//! emit equals one full `replace`. The crate has no other sensitive-text pattern, so this does not try
//! to apply a second redactor. `run_loop` still redacts the raw `Response.text`
//! after the stream ends; that pass is what stored messages use.

use super::model::ModelError;

/// Incremental `key` → `[REDACTED]` replacement.
///
/// A non-empty key holds back the last `key.len() - 1` bytes of unprocessed text
/// (more, when that cut would split a char) so a key divided across chunks is
/// never emitted. An empty key is passed through immediately.
pub struct RedactTail {
    key: String,
    /// Raw text not yet safe to emit. Never contains a completed replacement.
    buf: String,
}

impl RedactTail {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            buf: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &str, emit: &mut dyn FnMut(&str)) {
        self.buf.push_str(chunk);
        self.drain(false, emit);
    }

    pub fn finish(&mut self, emit: &mut dyn FnMut(&str)) {
        self.drain(true, emit);
    }

    fn drain(&mut self, finishing: bool, emit: &mut dyn FnMut(&str)) {
        if self.key.is_empty() {
            emit_nonempty(&self.buf, emit);
            self.buf.clear();
            return;
        }
        // Replace only in the raw buffer. Holding a tail of `[REDACTED]` and
        // scanning it again would redact a key that happens to occur inside the marker.
        while let Some(pos) = self.buf.find(&self.key) {
            let rest = self.buf.split_off(pos + self.key.len());
            self.buf.truncate(pos);
            emit_nonempty(&self.buf, emit);
            emit_nonempty("[REDACTED]", emit);
            self.buf = rest;
        }
        if finishing {
            emit_nonempty(&self.buf, emit);
            self.buf.clear();
            return;
        }
        let hold = self.key.len() - 1;
        if self.buf.len() <= hold {
            return;
        }
        let mut cut = self.buf.len() - hold;
        while cut > 0 && !self.buf.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            return;
        }
        let rest = self.buf.split_off(cut);
        emit_nonempty(&self.buf, emit);
        self.buf = rest;
    }
}

fn emit_nonempty(piece: &str, emit: &mut dyn FnMut(&str)) {
    if !piece.is_empty() {
        emit(piece);
    }
}

/// Incremental SSE decoder. Lines may end in `\n` or `\r\n`.
///
/// `data:` values lose one leading space. Several `data:` lines become one
/// payload joined by `\n`. `event:` sets the name (one leading space removed,
/// empty when the field was absent). A blank line dispatches; [`SseParser::finish`]
/// dispatches a trailing event that never got a blank line. `on_event` returning
/// `Ok(false)` stops the parse.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            event: String::new(),
            data: Vec::new(),
        }
    }

    pub fn push(
        &mut self,
        bytes: &[u8],
        on_event: &mut dyn FnMut(&str, &str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        self.buf.extend_from_slice(bytes);
        while let Some(line) = next_line(&mut self.buf)? {
            if !self.handle_line(&line, on_event)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn finish(
        &mut self,
        on_event: &mut dyn FnMut(&str, &str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        if !self.buf.is_empty() {
            let mut raw = std::mem::take(&mut self.buf);
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            let line = String::from_utf8(raw)
                .map_err(|e| ModelError::FormatError(format!("invalid UTF-8 in SSE line: {e}")))?;
            if !self.handle_line(&line, on_event)? {
                return Ok(false);
            }
        }
        self.flush_event(on_event)
    }

    fn handle_line(
        &mut self,
        line: &str,
        on_event: &mut dyn FnMut(&str, &str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        if line.is_empty() {
            return self.flush_event(on_event);
        }
        if line.starts_with(':') {
            return Ok(true);
        }
        let (field, value) = match line.split_once(':') {
            Some((field, rest)) => (field, rest.strip_prefix(' ').unwrap_or(rest)),
            None => (line, ""),
        };
        match field {
            "data" => self.data.push(value.to_string()),
            "event" => self.event = value.to_string(),
            _ => {}
        }
        Ok(true)
    }

    fn flush_event(
        &mut self,
        on_event: &mut dyn FnMut(&str, &str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        if self.data.is_empty() {
            self.event.clear();
            return Ok(true);
        }
        let name = std::mem::take(&mut self.event);
        let data = std::mem::take(&mut self.data).join("\n");
        on_event(&name, &data)
    }
}

/// Take one complete line, stripping a trailing `\r`. `None` when no `\n` yet.
fn next_line(buf: &mut Vec<u8>) -> Result<Option<String>, ModelError> {
    let Some(nl) = buf.iter().position(|b| *b == b'\n') else {
        return Ok(None);
    };
    let rest = buf.split_off(nl + 1);
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    let line = std::mem::replace(buf, rest);
    match String::from_utf8(line) {
        Ok(text) => Ok(Some(text)),
        Err(e) => Err(ModelError::FormatError(format!(
            "invalid UTF-8 in SSE line: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(key: &str, text: &str, width_chars: usize) -> String {
        let mut tail = RedactTail::new(key);
        let mut out = String::new();
        // "ED" occurs inside the marker itself, so a contains-check would be a false leak.
        let marker_holds_key = !key.is_empty() && "[REDACTED]".contains(key);
        let mut accept = |part: &str| {
            assert!(!part.is_empty(), "empty emit");
            if !key.is_empty() && !marker_holds_key {
                assert!(!part.contains(key), "key leaked in {part:?}");
            }
            out.push_str(part);
        };
        let chars: Vec<char> = text.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            let end = (index + width_chars).min(chars.len());
            let piece: String = chars[index..end].iter().collect();
            tail.push(&piece, &mut accept);
            index = end;
        }
        tail.finish(&mut accept);
        out
    }

    fn assert_chunked(key: &str, text: &str) {
        let expect = if key.is_empty() {
            text.to_string()
        } else {
            text.replace(key, "[REDACTED]")
        };
        let widths = text.chars().count().max(1);
        for width in 1..=widths {
            assert_eq!(
                feed(key, text, width),
                expect,
                "key {key:?} width {width} text {text:?}"
            );
        }
    }

    #[test]
    fn key_split_across_chunks_never_appears() {
        let key = "SECRET";
        let mut tail = RedactTail::new(key);
        let mut parts = Vec::new();
        for chunk in ["hello SE", "CR", "ET!"] {
            tail.push(chunk, &mut |part| {
                assert!(!part.contains(key), "leak: {part}");
                assert!(!part.is_empty());
                parts.push(part.to_string());
            });
        }
        tail.finish(&mut |part| {
            assert!(!part.contains(key), "leak: {part}");
            parts.push(part.to_string());
        });
        let joined = parts.concat();
        assert_eq!(joined, "hello SECRET!".replace(key, "[REDACTED]"));
        assert!(!joined.contains(key));
        assert_chunked(key, "hello SECRET! and SECRET again");
    }

    #[test]
    fn concat_equals_full_replace() {
        // "ED" sits inside the marker `[REDACTED]`; a rescan would corrupt the result.
        assert_chunked("ED", "ED stays visible? no ED");
        assert_chunked("sk-test", "pre sk-te st post sk-test end");
        assert_chunked("tok", "tok a toké tok to tokentok tail");
        assert_chunked("aa", "aaa aaaa");
    }

    #[test]
    fn empty_key_emits_immediately() {
        let mut tail = RedactTail::new("");
        let mut parts = Vec::new();
        tail.push("ab", &mut |part| parts.push(part.to_string()));
        assert_eq!(parts, vec!["ab".to_string()]);
        tail.push("", &mut |part| parts.push(part.to_string()));
        assert_eq!(parts, vec!["ab".to_string()]);
        tail.push("c", &mut |part| parts.push(part.to_string()));
        tail.finish(&mut |part| parts.push(part.to_string()));
        assert_eq!(parts, vec!["ab".to_string(), "c".to_string()]);
        assert_chunked("", "plain text");
    }

    #[test]
    fn multibyte_char_boundary_does_not_split() {
        // Hold is 1 byte; "€" is 3 bytes, so a naive cut lands inside the char.
        let mut tail = RedactTail::new("zz");
        let mut out = String::new();
        tail.push("a€", &mut |part| out.push_str(part));
        tail.push("b", &mut |part| out.push_str(part));
        tail.finish(&mut |part| out.push_str(part));
        assert_eq!(out, "a€b");

        // "éX" is split across pushes. A lone "é" may be emitted once the next
        // chunk shows it is not the key; the key itself must not be.
        let mut tail = RedactTail::new("éX");
        let mut out = String::new();
        for chunk in ["pré", "é", "X!"] {
            tail.push(chunk, &mut |part| {
                assert!(!part.contains("éX"), "key leaked: {part}");
                out.push_str(part);
            });
        }
        tail.finish(&mut |part| {
            assert!(!part.contains("éX"), "key leaked: {part}");
            out.push_str(part);
        });
        assert_eq!(out, "prééX!".replace("éX", "[REDACTED]"));
        assert_chunked("éX", "a€ éX b€ éX");
        assert_chunked("zz", "a€b€c");
    }

    fn collect(bytes: &[u8]) -> Result<Vec<(String, String)>, ModelError> {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        let mut on_event = |name: &str, data: &str| {
            events.push((name.to_string(), data.to_string()));
            Ok(true)
        };
        if parser.push(bytes, &mut on_event)? {
            parser.finish(&mut on_event)?;
        }
        Ok(events)
    }

    #[test]
    fn sse_crlf_comment_and_event_name() {
        let events = collect(b"event: delta\r\n: keep-alive\r\ndata: hi\r\n\r\n").unwrap();
        assert_eq!(events, vec![("delta".to_string(), "hi".to_string())]);
    }

    #[test]
    fn sse_comments_are_ignored() {
        let events = collect(b": this is not data\n: second\ndata: real\n\n").unwrap();
        assert_eq!(events, vec![("".to_string(), "real".to_string())]);
    }

    #[test]
    fn sse_multiple_data_lines_join_with_newline() {
        let events = collect(b"data: one\ndata: two\ndata:  three\n\n").unwrap();
        assert_eq!(
            events,
            vec![("".to_string(), "one\ntwo\n three".to_string())]
        );
    }

    #[test]
    fn sse_trailing_event_without_blank_line() {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        parser
            .push(b"event: end\ndata: tail\n", &mut |name, data| {
                events.push((name.to_string(), data.to_string()));
                Ok(true)
            })
            .unwrap();
        assert!(events.is_empty());
        parser
            .finish(&mut |name, data| {
                events.push((name.to_string(), data.to_string()));
                Ok(true)
            })
            .unwrap();
        assert_eq!(events, vec![("end".to_string(), "tail".to_string())]);

        let mut parser = SseParser::new();
        let mut events = Vec::new();
        parser
            .push(b"event: end\ndata: tail", &mut |_, _| Ok(true))
            .unwrap();
        parser
            .finish(&mut |name, data| {
                events.push((name.to_string(), data.to_string()));
                Ok(true)
            })
            .unwrap();
        assert_eq!(events, vec![("end".to_string(), "tail".to_string())]);
    }

    #[test]
    fn sse_split_across_pushes_including_utf8() {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        assert!(parser.push(b"data: hel", &mut |_, _| Ok(true)).unwrap());
        assert!(events.is_empty());
        assert!(parser
            .push(b"lo\n\n", &mut |name, data| {
                events.push((name.to_string(), data.to_string()));
                Ok(true)
            })
            .unwrap());
        assert_eq!(events, vec![("".to_string(), "hello".to_string())]);

        let mut parser = SseParser::new();
        let mut events = Vec::new();
        // "é" is C3 A9, split between the two bytes. The first push has no newline yet.
        assert!(parser.push(b"data: \xc3", &mut |_, _| Ok(true)).unwrap());
        assert!(events.is_empty());
        assert!(parser
            .push(b"\xa9\n\n", &mut |name, data| {
                events.push((name.to_string(), data.to_string()));
                Ok(true)
            })
            .unwrap());
        assert_eq!(events, vec![("".to_string(), "é".to_string())]);
    }

    #[test]
    fn sse_stop_false_halts() {
        let mut parser = SseParser::new();
        let mut seen = Vec::new();
        let keep_going = parser
            .push(b"data: one\n\ndata: two\n\n", &mut |_, data| {
                seen.push(data.to_string());
                Ok(false)
            })
            .unwrap();
        assert!(!keep_going);
        assert_eq!(seen, vec!["one".to_string()]);
    }

    #[test]
    fn sse_invalid_utf8_is_format_error() {
        let mut parser = SseParser::new();
        assert!(parser.push(&[0xff], &mut |_, _| Ok(true)).is_ok());
        let err = parser
            .push(b"\n", &mut |_, _| Ok(true))
            .expect_err("complete line of invalid UTF-8");
        match err {
            ModelError::FormatError(msg) => assert!(msg.contains("UTF-8"), "{msg}"),
            other => panic!("expected FormatError, got {other}"),
        }
    }
}
