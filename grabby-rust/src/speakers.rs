//! Speaker labels in transcripts (port of `components/speakers.ts`) and search snippet
//! parsing (`components/searchSnippet.ts`).

#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerEntry {
    /// e.g. "Speaker 1"
    pub original: String,
    /// The edited name.
    pub current: String,
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Byte offsets where a line starts (JavaScript's multiline `^`).
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in text.char_indices() {
        if is_line_break(c) {
            starts.push(i + c.len_utf8());
        }
    }
    starts
}

/// The label before the first colon of a line, when the line has one: `^([^:\r\n]+):`.
fn label_at(text: &str, start: usize) -> Option<&str> {
    let rest = &text[start..];
    let end = rest.find(|c: char| c == ':' || c == '\r' || c == '\n')?;
    (end > 0 && rest[end..].starts_with(':')).then(|| &rest[..end])
}

/// Extract the unique speaker labels from transcript text, sorted.
pub fn extract_speakers(text: &str) -> Vec<SpeakerEntry> {
    let mut seen: Vec<String> = Vec::new();
    for start in line_starts(text) {
        if let Some(label) = label_at(text, start) {
            if !seen.iter().any(|s| s == label) {
                seen.push(label.to_owned());
            }
        }
    }
    seen.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    seen.into_iter().map(|s| SpeakerEntry { original: s.clone(), current: s }).collect()
}

/// Replace "OldName:" at the start of every line with "NewName:".
pub fn replace_speaker(text: &str, old_name: &str, new_name: &str) -> String {
    let prefix = format!("{old_name}:");
    let mut out = String::with_capacity(text.len());
    let starts = line_starts(text);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(text.len());
        let line = &text[start..end];
        match line.strip_prefix(&prefix) {
            Some(rest) => {
                out.push_str(new_name);
                out.push(':');
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Splits a line into its speaker label and the rest (`^(.+?):(.*)`).
pub fn split_label(line: &str) -> Option<(&str, &str)> {
    let i = line.find(':')?;
    (i > 0).then(|| (&line[..i], &line[i + 1..]))
}

/// Turns an FTS snippet with `**match**` markers into plain text and highlighted byte ranges.
pub fn parse_snippet(snippet: &str) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut marks = Vec::new();
    let mut rest = snippet;
    while let Some(open) = rest.find("**") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("**") else { break };
        text.push_str(&rest[..open]);
        let start = text.len();
        text.push_str(&after[..close]);
        marks.push((start, text.len()));
        rest = &after[close + 2..];
    }
    text.push_str(rest);
    (text, marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_replaces_speakers() {
        let text = "Speaker 2: hi\nSpeaker 1: hello: there\nno label\nSpeaker 2: again";
        let speakers = extract_speakers(text);
        assert_eq!(speakers.iter().map(|s| s.original.as_str()).collect::<Vec<_>>(), ["Speaker 1", "Speaker 2"]);
        assert_eq!(
            replace_speaker(text, "Speaker 2", "Ana"),
            "Ana: hi\nSpeaker 1: hello: there\nno label\nAna: again"
        );
        assert_eq!(replace_speaker("Speaker 22: x", "Speaker 2", "Ana"), "Speaker 22: x");
    }

    #[test]
    fn parses_snippets() {
        let (text, marks) = parse_snippet("…we **review** the **roadmap**…");
        assert_eq!(text, "…we review the roadmap…");
        assert_eq!(marks.iter().map(|&(a, b)| &text[a..b]).collect::<Vec<_>>(), ["review", "roadmap"]);
    }
}
