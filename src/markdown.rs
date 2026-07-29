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

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Styling that does not assume a background colour.
///
/// tui-markdown's defaults set backgrounds without setting a matching
/// foreground — an H1 is `on_cyan().bold().underlined()`, so the text keeps
/// whatever the terminal's default foreground is. That is white on cyan in a
/// dark theme and dark grey on cyan in a light one, and both are hard to read.
/// Inline code is `white().on_black()`, a fixed black block that is equally
/// wrong on a light background.
///
/// Structure is carried by modifiers instead, which work in any theme, with a
/// single foreground accent for code. Same rule as the rest of the UI: the
/// terminal owns colour.
#[derive(Clone)]
struct Adaptive;

impl tui_markdown::StyleSheet for Adaptive {
    fn heading(&self, level: u8) -> Style {
        // Depth reads from weight and decoration rather than colour.
        match level {
            1 => Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            2 => Style::default().add_modifier(Modifier::BOLD),
            _ => Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::ITALIC),
        }
    }

    fn code(&self) -> Style {
        Style::default().fg(ratatui::style::Color::Cyan)
    }

    fn link(&self) -> Style {
        Style::default().add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::default()
            .fg(ratatui::style::Color::DarkGray)
            .add_modifier(Modifier::ITALIC)
    }

    fn table_border(&self) -> Style {
        Style::default().fg(ratatui::style::Color::DarkGray)
    }

    fn table_header(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Renders `text`, honouring single newlines as line breaks.
///
/// Returns owned lines: the soft-break pass produces a new String, and borrowed
/// output could not outlive it.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let prepared = hard_break_soft_lines(text);

    let options = tui_markdown::Options::new(Adaptive);

    tui_markdown::from_str_with_options(&prepared, &options)
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


#[cfg(test)]
mod style_checks {
    use super::*;
    use ratatui::style::Style;

    /// A style that sets a background but no foreground leaves the text in the
    /// terminal's default colour, which is unreadable against the background in
    /// one theme or the other. That is the bug this stylesheet exists to fix.
    fn no_bare_background(style: Style, what: &str) {
        if style.bg.is_some() {
            assert!(
                style.fg.is_some(),
                "{what} sets a background with no foreground: {style:?}"
            );
        }
    }

    #[test]
    fn no_rendered_style_sets_a_background_without_a_foreground() {
        let src = "# H1\n\n## H2\n\n### H3\n\ntext with `code` in it\n\n\
                   > a quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
                   [link](https://example.com)\n";

        for line in render(src) {
            no_bare_background(line.style, "line");
            for span in &line.spans {
                no_bare_background(span.style, &format!("span {:?}", span.content));
            }
        }
    }

    #[test]
    fn headings_are_distinguished_without_relying_on_colour() {
        // Modifiers survive any terminal theme; a colour choice may not.
        use tui_markdown::StyleSheet;
        let sheet = Adaptive;
        for level in 1..=3u8 {
            let s = sheet.heading(level);
            assert!(
                !s.add_modifier.is_empty(),
                "heading {level} has no modifier to distinguish it: {s:?}"
            );
            assert!(s.bg.is_none(), "heading {level} sets a background");
        }
    }

    #[test]
    fn inline_code_has_no_fixed_black_block() {
        use tui_markdown::StyleSheet;
        assert!(Adaptive.code().bg.is_none());
    }
}

