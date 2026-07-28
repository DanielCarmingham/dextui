//! A deliberately small markdown reader for task descriptions.
//!
//! dex stores descriptions as plain strings and renders them with whitespace
//! collapsed, but the text people write is markdown -- `dex plan` reads markdown
//! files, and dex syncs descriptions into GitHub issue bodies where markdown does
//! render. So the source is reliably markdown-ish and worth styling.
//!
//! This covers headings, fenced code, list markers, blockquotes, inline code and
//! bold. Not supported, on purpose: italics (ambiguous with list markers and
//! snake_case), links, tables (styling them would need column measurement), and
//! nested emphasis. Anything unrecognised is passed through as plain text, so no
//! input can ever be mangled or lost.
//!
//! Emitting a neutral `Emphasis` rather than a ratatui `Style` keeps this
//! testable and leaves all colour decisions in one place (`ui.rs`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Plain,
    /// Syntax itself: `#`, `-`, `>`, the backticks of a fence.
    Marker,
    Heading,
    Bold,
    /// Inline `code`.
    Code,
    /// A line inside a fenced block.
    CodeBlock,
    Quote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub emphasis: Emphasis,
}

impl Segment {
    fn new(text: impl Into<String>, emphasis: Emphasis) -> Self {
        Self {
            text: text.into(),
            emphasis,
        }
    }
}

/// Parses into one vector of segments per source line. Line count is always
/// preserved, so the caller can rely on the layout being unchanged.
pub fn parse(text: &str) -> Vec<Vec<Segment>> {
    let mut out = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        // Fence delimiters toggle the block and are shown as syntax.
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push(vec![Segment::new(line, Emphasis::Marker)]);
            continue;
        }

        if in_fence {
            // Verbatim: indentation inside code is meaningful.
            out.push(vec![Segment::new(line, Emphasis::CodeBlock)]);
            continue;
        }

        out.push(parse_line(line));
    }

    out
}

fn parse_line(line: &str) -> Vec<Segment> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let mut segments = Vec::new();

    if !indent.is_empty() {
        segments.push(Segment::new(indent, Emphasis::Plain));
    }

    // Heading: one to six #, then a space.
    if let Some(hashes) = heading_marker(rest) {
        segments.push(Segment::new(&rest[..hashes], Emphasis::Marker));
        segments.push(Segment::new(&rest[hashes..], Emphasis::Heading));
        return segments;
    }

    // Blockquote.
    if let Some(body) = rest.strip_prefix("> ") {
        segments.push(Segment::new("> ", Emphasis::Marker));
        segments.push(Segment::new(body, Emphasis::Quote));
        return segments;
    }

    // List marker: bullet or ordered.
    if let Some(marker_len) = list_marker(rest) {
        segments.push(Segment::new(&rest[..marker_len], Emphasis::Marker));
        segments.extend(parse_inline(&rest[marker_len..]));
        return segments;
    }

    segments.extend(parse_inline(rest));
    segments
}

fn heading_marker(s: &str) -> Option<usize> {
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && s.chars().nth(hashes) == Some(' ') {
        Some(hashes + 1)
    } else {
        None
    }
}

fn list_marker(s: &str) -> Option<usize> {
    for prefix in ["- ", "* ", "+ "] {
        if s.starts_with(prefix) {
            return Some(2);
        }
    }

    // Ordered: digits, then `.` or `)`, then a space.
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return Some(digits + 2);
        }
    }

    None
}

/// Handles `` `code` `` and `**bold**`. Unterminated delimiters stay literal.
fn parse_inline(s: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < s.len() {
        // Delimiters are kept and dimmed rather than stripped, matching how
        // block-level syntax (`#`, `-`, `>`) is handled. It also means no input
        // character is ever dropped, which the round-trip test enforces.
        if bytes[i] == b'`'
            && let Some(end) = find_close(s, i + 1, "`") {
                flush(&mut plain, &mut out);
                out.push(Segment::new("`", Emphasis::Marker));
                out.push(Segment::new(&s[i + 1..end], Emphasis::Code));
                out.push(Segment::new("`", Emphasis::Marker));
                i = end + 1;
                continue;
            }

        if s[i..].starts_with("**")
            && let Some(end) = find_close(s, i + 2, "**") {
                flush(&mut plain, &mut out);
                out.push(Segment::new("**", Emphasis::Marker));
                out.push(Segment::new(&s[i + 2..end], Emphasis::Bold));
                out.push(Segment::new("**", Emphasis::Marker));
                i = end + 2;
                continue;
            }

        // Step by whole characters so multi-byte text is never split.
        let ch_len = s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        plain.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }

    flush(&mut plain, &mut out);
    out
}

fn find_close(s: &str, from: usize, delim: &str) -> Option<usize> {
    if from > s.len() {
        return None;
    }
    s[from..].find(delim).map(|rel| from + rel)
}

fn flush(plain: &mut String, out: &mut Vec<Segment>) {
    if !plain.is_empty() {
        out.push(Segment::new(std::mem::take(plain), Emphasis::Plain));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &[Segment]) -> Vec<&str> {
        line.iter().map(|s| s.text.as_str()).collect()
    }

    fn kinds(line: &[Segment]) -> Vec<Emphasis> {
        line.iter().map(|s| s.emphasis).collect()
    }

    #[test]
    fn line_count_is_always_preserved() {
        // The caller relies on this: styling must never change the layout.
        let text = "one\n\n# two\n```\ncode\n```\n- four";
        assert_eq!(parse(text).len(), text.lines().count());
    }

    #[test]
    fn heading_marks_are_separated_from_the_text() {
        let lines = parse("## Some heading");
        assert_eq!(texts(&lines[0]), vec!["## ", "Some heading"]);
        assert_eq!(kinds(&lines[0]), vec![Emphasis::Marker, Emphasis::Heading]);
    }

    #[test]
    fn seven_hashes_is_not_a_heading() {
        let lines = parse("####### not a heading");
        assert_eq!(kinds(&lines[0]), vec![Emphasis::Plain]);
    }

    #[test]
    fn hash_without_a_space_is_not_a_heading() {
        let lines = parse("#hashtag");
        assert_eq!(kinds(&lines[0]), vec![Emphasis::Plain]);
    }

    #[test]
    fn fenced_blocks_are_verbatim_including_indentation() {
        let lines = parse("```rust\n    indented();\n```");
        assert_eq!(kinds(&lines[0]), vec![Emphasis::Marker]);
        assert_eq!(texts(&lines[1]), vec!["    indented();"]);
        assert_eq!(kinds(&lines[1]), vec![Emphasis::CodeBlock]);
        assert_eq!(kinds(&lines[2]), vec![Emphasis::Marker]);
    }

    #[test]
    fn markdown_inside_a_fence_is_not_interpreted() {
        let lines = parse("```\n# not a heading\n```");
        assert_eq!(kinds(&lines[1]), vec![Emphasis::CodeBlock]);
    }

    #[test]
    fn nested_list_indentation_is_preserved() {
        let lines = parse("  - nested");
        assert_eq!(texts(&lines[0]), vec!["  ", "- ", "nested"]);
    }

    #[test]
    fn ordered_list_markers_are_recognised() {
        assert_eq!(texts(&parse("1. first")[0]), vec!["1. ", "first"]);
        assert_eq!(texts(&parse("12) twelfth")[0]), vec!["12) ", "twelfth"]);
    }

    #[test]
    fn inline_code_and_bold_keep_their_delimiters_as_markers() {
        let lines = parse("run `cargo test` and **stop**");
        assert_eq!(
            texts(&lines[0]),
            vec!["run ", "`", "cargo test", "`", " and ", "**", "stop", "**"]
        );
        assert_eq!(
            kinds(&lines[0]),
            vec![
                Emphasis::Plain,
                Emphasis::Marker,
                Emphasis::Code,
                Emphasis::Marker,
                Emphasis::Plain,
                Emphasis::Marker,
                Emphasis::Bold,
                Emphasis::Marker,
            ]
        );
    }

    #[test]
    fn unterminated_delimiters_stay_literal() {
        // Nothing may be swallowed just because the syntax was incomplete.
        let lines = parse("a ` dangling and ** unclosed");
        assert_eq!(texts(&lines[0]).concat(), "a ` dangling and ** unclosed");
    }

    #[test]
    fn text_is_never_lost_or_reordered() {
        let src = "# H\n\n- a **b** `c`\n  - nested\n> quote\n```\nfn x() {}\n```\nplain";
        for (parsed, original) in parse(src).iter().zip(src.lines()) {
            let rebuilt: String = parsed.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(rebuilt, original);
        }
    }

    #[test]
    fn multibyte_text_is_not_split() {
        let src = "héllo **wörld** — ok";
        let lines = parse(src);
        assert_eq!(texts(&lines[0]).concat(), src);
        // The accented word must survive as one bold segment, not be cut mid-char.
        assert!(lines[0]
            .iter()
            .any(|s| s.emphasis == Emphasis::Bold && s.text == "wörld"));
    }

    #[test]
    fn blockquotes_separate_the_marker() {
        let lines = parse("> quoted");
        assert_eq!(kinds(&lines[0]), vec![Emphasis::Marker, Emphasis::Quote]);
    }
}
