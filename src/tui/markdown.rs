//! WP-602: markdown rendering for transcript messages — the upstream visual
//! contract for the supported subset (no syntax highlighting, no math),
//! WP-607: colors come from the active theme (upstream token names):
//!
//! - headings `#`..`######`   → bold `mdHeading`
//! - lists `-`/`*`/`1.`      → `mdListBullet` bullet, nested by leading spaces
//! - fenced code ```` ```lang ```` → `mdCodeBlockBorder` label + `mdCodeBlock` lines
//! - inline `code`           → `mdCode`
//! - **bold** / *italic* / [text](url) → bold / italic / `mdLink` + `mdLinkUrl`
//!   (~~strike~~ stays literal: ratatui 0.30 has no strikethrough modifier)
//! - blockquote `>`          → `mdQuoteBorder` border + italic `mdQuote`
//!
//! Pure function over the message text: `md_lines` returns styled ratatui
//! Lines (the pane applies word wrap). Incomplete markup degrades to plain
//! text (streaming-safe, WP-603 refines partial rendering).
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme::Theme;

/// Render one markdown message into styled lines (no wrapping applied here).
pub fn md_lines(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;

    for raw in text.lines() {
        let line = raw;
        let fence = line.trim_start().starts_with("```");
        if fence {
            if in_code {
                in_code = false;
            } else {
                in_code = true;
                let lang = line
                    .trim_start()
                    .trim_start_matches("```")
                    .trim()
                    .to_string();
                if !lang.is_empty() {
                    out.push(Line::from(Span::styled(
                        lang,
                        theme.fg("mdCodeBlockBorder"),
                    )));
                }
            }
            continue;
        }
        if in_code {
            // Code lines keep their text; the whole block is green.
            out.push(Line::from(Span::styled(
                line.to_string(),
                theme.fg("mdCodeBlock"),
            )));
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            out.push(Line::from(Span::raw("")));
            continue;
        }
        // Heading: 1-6 '#' then whitespace.
        let hashes: usize = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            let content = inline(trimmed[hashes + 1..].trim(), theme);
            let style = theme.fg_bold("mdHeading");
            out.push(Line::from({
                let mut spans = vec![Span::styled(format!("{hashes} "), theme.fg("muted"))];
                spans.extend(content.into_iter().map(|mut s| {
                    s.style = s.style.patch(style);
                    s
                }));
                spans
            }));
            continue;
        }
        // Blockquote: '>' border (dim) + italic content.
        if let Some(rest) = trimmed.strip_prefix('>') {
            let content = inline(rest.trim(), theme);
            out.push(Line::from({
                let mut spans = vec![Span::styled("> ".to_string(), theme.fg("mdQuoteBorder"))];
                spans.extend(content.into_iter().map(|mut s| {
                    s.style = s
                        .style
                        .patch(theme.fg("mdQuote"))
                        .add_modifier(Modifier::ITALIC);
                    s
                }));
                spans
            }));
            continue;
        }
        // List item: '- ', '* ', '+ ', 'N. ' / 'N) ' — nesting by 2-space steps.
        let indent = (line.len() - trimmed.len()) / 2;
        let is_marker = |m: &str| {
            m == "-"
                || m == "*"
                || m == "+"
                || m.trim_end_matches(['.', ')'])
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    && !m.is_empty()
                    && m.ends_with(['.', ')'])
        };
        let (marker, rest) = match trimmed.split_once(' ') {
            Some((m, r)) if is_marker(m) => (Some(m), r),
            _ => (None, trimmed),
        };
        match marker {
            Some(m) => {
                let content = inline(rest.trim(), theme);
                let mut spans: Vec<Span> = vec![
                    Span::raw("  ".repeat(indent.min(4))),
                    Span::styled(format!("{m} "), theme.fg("mdListBullet")),
                ];
                spans.extend(content);
                out.push(Line::from(spans));
            }
            None => out.push(Line::from(inline(trimmed, theme))),
        }
    }
    out
}

/// Inline styles: `code`, **bold**, *italic* (and _italic_), ~~strike~~,
/// [text](url). Flat (no nested markup inside markup — shortcut: upgrade to
/// a real inline parser if nested markup becomes common in model output).
fn inline(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut plain: String = String::new();

    let flush = |spans: &mut Vec<Span<'static>>, plain: &mut String| {
        if !plain.is_empty() {
            spans.push(Span::raw(std::mem::take(plain)));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().collect();
        // `code`
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&ch| ch == '`') {
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                flush(&mut spans, &mut plain);
                spans.push(Span::styled(format!("`{code}`"), theme.fg("mdCode")));
                i += end + 2;
                continue;
            }
        }
        // **bold** (~~strike~~ is not parsable: ratatui 0.30 has no
        // strikethrough modifier, so it stays literal)
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = rest[2..].find("**") {
                let inner: String = chars[i + 2..i + 2 + end].iter().collect();
                flush(&mut spans, &mut plain);
                spans.push(Span::styled(
                    inner,
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                i += end + 4;
                continue;
            }
        }
        // *italic* / _italic_
        if c == '*' || c == '_' {
            if let Some(end) = chars[i + 1..].iter().position(|&ch| ch == c) {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if !inner.is_empty() && !inner.ends_with(' ') {
                    flush(&mut spans, &mut plain);
                    spans.push(Span::styled(
                        inner,
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                    i += end + 2;
                    continue;
                }
            }
        }
        // [text](url) → blue text + dim url (upstream: link + linkUrl).
        if c == '[' {
            if let Some(close) = chars[i + 1..].iter().position(|&ch| ch == ']') {
                let close_abs = i + 1 + close; // index of ']'
                let paren_abs = close_abs + 1; // index of '('
                if chars.get(paren_abs) == Some(&'(') {
                    if let Some(url_end) = chars[paren_abs + 1..].iter().position(|&ch| ch == ')') {
                        let link_text: String = chars[i + 1..close_abs].iter().collect();
                        let url: String = chars[paren_abs + 1..paren_abs + 1 + url_end]
                            .iter()
                            .collect();
                        flush(&mut spans, &mut plain);
                        spans.push(Span::styled(link_text, theme.fg("mdLink")));
                        spans.push(Span::styled(format!(" ({url})"), theme.fg("mdLinkUrl")));
                        i = paren_abs + 1 + url_end + 1;
                        continue;
                    }
                }
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut spans, &mut plain);
    spans
}

/// Plain (non-markdown) line, e.g. tool summaries.
pub fn plain_line(text: &str) -> Line<'static> {
    Line::from(Span::raw(text.to_string()))
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::tui::theme::Theme;
    use ratatui::style::Color;

    /// Golden test theme: the built-in dark.
    fn dark() -> Theme {
        Theme::builtin("dark").unwrap()
    }

    fn spans_of<'a>(lines: &'a [Line<'a>], idx: usize) -> Vec<Span<'a>> {
        lines[idx].spans.clone()
    }

    fn flat_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn headings_are_bold_md_heading() {
        let theme = dark();
        let lines = md_lines("# Title\n## Sub", &theme);
        assert_eq!(lines.len(), 2);
        let t = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title")
            .unwrap();
        // Golden (dark): mdHeading = yellow var = okhsl(83 88% 67%).
        assert_eq!(t.style.fg, Some(Color::Rgb(205, 154, 34)));
        assert!(t.style.add_modifier.contains(Modifier::BOLD));
        let t2 = lines[1]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Sub")
            .unwrap();
        assert_eq!(t2.style.fg, Some(Color::Rgb(205, 154, 34)));
        assert!(t2.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn list_items_get_theme_bullets_and_indent() {
        let theme = dark();
        let lines = md_lines("- one\n- two\n  - nested\n1. numbered", &theme);
        assert_eq!(lines.len(), 4);
        let l0 = &lines[0];
        // Golden (dark): mdListBullet = violet var = okhsl(295 50% 67%).
        assert!(l0
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "- " && s.style.fg == Some(Color::Rgb(167, 152, 215))));
        assert_eq!(
            flat_text(&lines[0..1]),
            "- one".to_string() + "\n".trim_end()
        );
        let nested = &lines[2];
        assert!(
            nested
                .spans
                .first()
                .map(|s| s.content.as_ref().contains(' '))
                .unwrap_or(false),
            "nested item is indented: {nested:?}"
        );
        let numbered = &lines[3];
        assert!(numbered.spans.iter().any(|s| s.content.as_ref() == "1. "));
    }

    #[test]
    fn fenced_code_block_has_theme_label_and_code_lines() {
        let theme = dark();
        let lines = md_lines("```rust\nfn main() {}\n```", &theme);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(spans_of(&lines, 0)[0].content.as_ref(), "rust");
        // Golden (dark): mdCodeBlockBorder = muted = okhsl(229 6% 67%).
        assert_eq!(
            spans_of(&lines, 0)[0].style.fg,
            Some(Color::Rgb(157, 165, 169))
        );
        assert_eq!(spans_of(&lines, 1)[0].content.as_ref(), "fn main() {}");
        // Golden (dark): mdCodeBlock = green var = okhsl(159 59% 67%).
        assert_eq!(
            spans_of(&lines, 1)[0].style.fg,
            Some(Color::Rgb(104, 183, 141))
        );
    }

    #[test]
    fn unclosed_fence_degrades_to_code_until_end() {
        // Streaming-safe: an open fence renders the rest as code, never
        // crashes and never swallows the label.
        let theme = dark();
        let lines = md_lines("```py\nprint(1)", &theme);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(
            spans_of(&lines, 1)[0].style.fg,
            Some(Color::Rgb(104, 183, 141))
        );
    }

    #[test]
    fn inline_code_is_md_code() {
        let theme = dark();
        let lines = md_lines("use `cargo build` now", &theme);
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        let code = s
            .iter()
            .find(|s| s.content.as_ref() == "`cargo build`")
            .unwrap();
        // Golden (dark): mdCode = violet var = okhsl(295 50% 67%).
        assert_eq!(code.style.fg, Some(Color::Rgb(167, 152, 215)));
    }

    #[test]
    fn bold_and_italic() {
        let theme = dark();
        let lines = md_lines("a **b** c *d* e", &theme);
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        let b = s.iter().find(|s| s.content.as_ref() == "b").unwrap();
        assert!(b.style.add_modifier.contains(Modifier::BOLD));
        let d = s.iter().find(|s| s.content.as_ref() == "d").unwrap();
        assert!(d.style.add_modifier.contains(Modifier::ITALIC));
        assert!(s.iter().any(|s| s.content.as_ref() == "a "));
    }

    #[test]
    fn strikethrough_degrades_to_literal() {
        // ratatui 0.30 has no strikethrough modifier: ~~x~~ renders as-is.
        let theme = dark();
        let text = flat_text(&md_lines("keep ~~f~~ intact", &theme));
        assert!(text.contains("~~f~~"), "{text}");
    }

    #[test]
    fn links_render_theme_link_and_url() {
        let theme = dark();
        let lines = md_lines("see [docs](https://example.com/x) here", &theme);
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        let t = s.iter().find(|s| s.content.as_ref() == "docs").unwrap();
        // Golden (dark): mdLink = blue var = okhsl(232 54% 67%).
        assert_eq!(t.style.fg, Some(Color::Rgb(105, 173, 208)));
        let u = s
            .iter()
            .find(|s| s.content.as_ref() == " (https://example.com/x)")
            .unwrap();
        // Golden (dark): mdLinkUrl = muted var = okhsl(229 6% 67%).
        assert_eq!(u.style.fg, Some(Color::Rgb(157, 165, 169)));
    }

    #[test]
    fn blockquote_is_italic_with_theme_border() {
        let theme = dark();
        let lines = md_lines("> quoted words", &theme);
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        assert_eq!(s[0].content.as_ref(), "> ");
        // Golden (dark): mdQuoteBorder = muted var.
        assert_eq!(s[0].style.fg, Some(Color::Rgb(157, 165, 169)));
        let q = s
            .iter()
            .find(|s| s.content.as_ref() == "quoted words")
            .unwrap();
        assert!(q.style.add_modifier.contains(Modifier::ITALIC));
        // Golden (dark): mdQuote = muted var.
        assert_eq!(q.style.fg, Some(Color::Rgb(157, 165, 169)));
    }

    #[test]
    fn unmatched_markup_stays_literal() {
        let theme = dark();
        let lines = md_lines("2 * 3 = 6 and **unmatched", &theme);
        let text = flat_text(&lines);
        assert!(text.contains("2 * 3 = 6"), "{text}");
        assert!(text.contains("**unmatched"), "{text}");
    }

    #[test]
    fn mixed_document_snapshot() {
        let md = "# Plan\n- do `x` **now**\n  - detail\n```sh\necho hi\n```\n> note";
        let theme = dark();
        let text = flat_text(&md_lines(md, &theme));
        assert_eq!(
            text, "1 Plan\n- do `x` now\n  - detail\nsh\necho hi\n> note",
            "layout: {text}"
        );
    }

    #[test]
    fn cjk_and_emoji_width_in_buffer() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Widget};
        // 你好 = 2 wide chars (4 cells), 👍 = 2 cells.
        let theme = dark();
        let lines = md_lines("你好 👍 world", &theme);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        Paragraph::new(lines).render(Rect::new(0, 0, 20, 1), &mut buf);
        let row: String = buf.content.iter().map(|c| c.symbol().to_string()).collect();
        // 你好 (4 cells) + space + 👍 (2 cells) + space + world (5) = 13 used.
        // Wide chars leave a blank continuation cell in the buffer, so the
        // raw row reads "你 好  👍  world".
        let world_col = row.chars().position(|c| c == 'w').unwrap();
        assert_eq!(world_col, 8, "emoji takes 2 cells, CJK 2 each: {row:?}");
        assert!(row.starts_with('你'), "{row:?}");
    }
}
