//! Markdown rendering for task descriptions.
//!
//! Rendering itself is `tui-markdown`'s job. This module exists for one thing it
//! does not offer: treating a single newline as a line break.
//!
//! This is unconditional rather than a setting. Joining lines was a regression
//! introduced when tui-markdown replaced the hand-rolled parser, which preserved
//! lines one-to-one; a switch to turn the regression back on has no use.
//!
//! CommonMark joins consecutive lines into one paragraph, which is correct but
//! wrong for dex descriptions — people write them as plain text, one thought per
//! line, and `dex` stores them verbatim. Without this, "line one\nline two"
//! renders as "line one line two".

use ratatui::text::{Line, Span};

/// Renders `text`, honouring single newlines as line breaks.
///
/// Returns owned lines: the soft-break pass produces a new String, and borrowed
/// output could not outlive it.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let prepared = hard_break_soft_lines(text);

    tui_markdown::from_str(&prepared)
        .lines
        .into_iter()
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style))
                .collect();
            Line::from(spans).style(line.style)
        })
        .collect()
}

/// Appends the two trailing spaces that CommonMark reads as a hard break.
///
/// Skipped for anything where trailing whitespace would change meaning or where
/// a break makes no sense:
///
/// - inside fenced code blocks, where content is verbatim;
/// - the fence delimiters themselves;
/// - blank lines, which already separate paragraphs;
/// - lines followed by a blank line, where the break is redundant;
/// - lines that already end in two or more spaces.
fn hard_break_soft_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() + lines.len() * 2);
    let mut in_fence = false;

    for (i, line) in lines.iter().enumerate() {
        let is_fence = line.trim_start().starts_with("```");
        if is_fence {
            in_fence = !in_fence;
        }

        out.push_str(line);

        let next_is_text = lines
            .get(i + 1)
            .is_some_and(|next| !next.trim().is_empty());

        let needs_break = !in_fence
            && !is_fence
            && !line.trim().is_empty()
            && next_is_text
            && !line.ends_with("  ");

        if needs_break {
            out.push_str("  ");
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn consecutive_lines_stay_separate() {
        let out = text_of(&render("line one\nline two"));
        assert!(
            out.iter().any(|l| l.trim() == "line one"),
            "lines were joined: {out:?}"
        );
        assert!(out.iter().any(|l| l.trim() == "line two"), "{out:?}");
    }

    #[test]
    fn code_fences_are_never_touched() {
        // Trailing spaces inside code would be a real corruption.
        let src = "text\n```\nfn main() {\n    let x = 1;\n}\n```\nmore";
        let prepared = hard_break_soft_lines(src);

        assert!(prepared.contains("fn main() {\n"), "code line was modified");
        assert!(
            prepared.contains("    let x = 1;\n"),
            "indented code line was modified: {prepared:?}"
        );
        assert!(!prepared.contains("```  "), "fence delimiter was modified");
    }

    #[test]
    fn blank_lines_are_left_alone() {
        let prepared = hard_break_soft_lines("a\n\nb");
        assert!(!prepared.contains("a  \n\n"), "break added before a blank line");
    }

    #[test]
    fn a_line_already_ending_in_a_hard_break_is_not_doubled() {
        let prepared = hard_break_soft_lines("a  \nb");
        assert!(!prepared.contains("a    \n"), "break was applied twice");
    }

    #[test]
    fn tables_still_render_as_tables() {
        // The preprocessing must not disturb the construct it runs alongside.
        let out = text_of(&render("| a | b |\n|---|---|\n| 1 | 2 |"));
        let joined = out.join("\n");
        assert!(joined.contains('┌') && joined.contains('┼'), "{joined}");
    }

    #[test]
    fn no_input_text_is_lost() {
        let src = "alpha\nbeta\n\n- item\n- another\n\n```\ncode\n```\ngamma";
        let prepared = hard_break_soft_lines(src);
        for word in ["alpha", "beta", "item", "another", "code", "gamma"] {
            assert!(prepared.contains(word), "{word} went missing");
        }
    }

    #[test]
    fn line_count_is_preserved_by_the_preprocessing() {
        let src = "a\nb\n\nc\n```\nd\n```\ne";
        assert_eq!(
            hard_break_soft_lines(src).lines().count(),
            src.lines().count()
        );
    }
}
