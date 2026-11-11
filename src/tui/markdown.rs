//! WP-602: markdown rendering for transcript messages — the upstream visual
//! contract for the supported subset (no syntax highlighting, no math):
//!
//! - headings `#`..`######`   → bold cyan
//! - lists `-`/`*`/`1.`      → cyan bullet, nested by leading spaces
//! - fenced code ```` ```lang ```` → dim language label + green code lines
//! - inline `code`           → yellow
//! - **bold** / *italic* / [text](url) → bold / italic / blue text + dim url
//!   (~~strike~~ stays literal: ratatui 0.30 has no strikethrough modifier)
//! - blockquote `>`          → dim border + italic
//!
//! Pure function over the message text: `md_lines` returns styled ratatui
//! Lines (the pane applies word wrap). Incomplete markup degrades to plain
//! text (streaming-safe, WP-603 refines partial rendering).
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

fn base() -> Style {
    Style::default()
}

/// Render one markdown message into styled lines (no wrapping applied here).
pub fn md_lines(text: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let mut code_lang: Option<String> = None;

    for raw in text.lines() {
        let line = raw;
        let fence = line.trim_start().starts_with("```");
        if fence {
            if in_code {
                in_code = false;
            } else {
                in_code = true;
                let lang = line.trim_start().trim_start_matches("```").trim().to_string();
                if !lang.is_empty() {
                    code_lang = Some(lang.clone());
                    out.push(Line::from(Span::styled(
                        lang,
                        base().fg(Color::DarkGray),
                    )));
                } else {
                    code_lang = None;
                }
            }
            continue;
        }
        if in_code {
            // Code lines keep their text; the whole block is green.
            out.push(Line::from(Span::styled(
                line.to_string(),
                base().fg(Color::Green),
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
            let content = inline(trimmed[hashes + 1..].trim());
            let style = base().fg(Color::Cyan).add_modifier(Modifier::BOLD);
            out.push(Line::from({
                let mut spans = vec![Span::styled(
                    format!("{hashes} "),
                    base().fg(Color::DarkGray),
                )];
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
            let content = inline(rest.trim());
            out.push(Line::from({
                let mut spans = vec![Span::styled(
                    "> ".to_string(),
                    base().fg(Color::DarkGray),
                )];
                spans.extend(content.into_iter().map(|mut s| {
                    s.style = s.style.add_modifier(Modifier::ITALIC);
                    s
                }));
                spans
            }));
            continue;
        }
        // List item: '- ', '* ', '+ ', 'N. ' / 'N) ' — nesting by 2-space steps.
        let indent = (line.len() - trimmed.len()) / 2;
        let is_marker = |m: &str| {
            m == "-" || m == "*" || m == "+"
                || m
                    .trim_end_matches(['.', ')'])
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    && !m.is_empty()
                    && m.ends_with(|c| c == '.' || c == ')')
        };
        let (marker, rest) = match trimmed.split_once(' ') {
            Some((m, r)) if is_marker(m) => (Some(m), r),
            _ => (None, trimmed),
        };
        match marker {
            Some(m) => {
                let content = inline(rest.trim());
                let mut spans: Vec<Span> = vec![
                    Span::raw("  ".repeat(indent.min(4))),
                    Span::styled(
                        format!("{m} "),
                        base().fg(Color::Cyan),
                    ),
                ];
                spans.extend(content);
                out.push(Line::from(spans));
            }
            None => out.push(Line::from(inline(trimmed))),
        }
    }
    out
}

/// Inline styles: `code`, **bold**, *italic* (and _italic_), ~~strike~~,
/// [text](url). Flat (no nested markup inside markup — shortcut: upgrade to
/// a real inline parser if nested markup becomes common in model output).
fn inline(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut plain: String = String::new();

    let mut flush = |spans: &mut Vec<Span<'static>>, plain: &mut String| {
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
                spans.push(Span::styled(
                    format!("`{code}`"),
                    base().fg(Color::Yellow),
                ));
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
                spans.push(Span::styled(inner, base().add_modifier(Modifier::BOLD)));
                i += end + 4;
                continue;
            }
        }
        // *italic* / _italic_
        if (c == '*' || c == '_') {
            if let Some(end) = chars[i + 1..].iter().position(|&ch| ch == c) {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if !inner.is_empty() && !inner.ends_with(' ') {
                    flush(&mut spans, &mut plain);
                    spans.push(Span::styled(inner, base().add_modifier(Modifier::ITALIC)));
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
                    if let Some(url_end) = chars[paren_abs + 1..]
                        .iter()
                        .position(|&ch| ch == ')')
                    {
                        let link_text: String = chars[i + 1..close_abs].iter().collect();
                        let url: String = chars[paren_abs + 1..paren_abs + 1 + url_end]
                            .iter()
                            .collect();
                        flush(&mut spans, &mut plain);
                        spans.push(Span::styled(
                            link_text,
                            base().fg(Color::Blue),
                        ));
                        spans.push(Span::styled(
                            format!(" ({url})"),
                            base().fg(Color::DarkGray),
                        ));
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

    fn style_of(line: &Line<'_>, span: usize) -> Style {
        line.spans[span].style
    }

    #[test]
    fn headings_are_bold_cyan() {
        let lines = md_lines("# Title\n## Sub");
        assert_eq!(lines.len(), 2);
        assert!(spans_of(&lines, 0).iter().any(|s| s.content.as_ref() == "Title"));
        let t = lines[0].spans.iter().find(|s| s.content.as_ref() == "Title").unwrap();
        assert_eq!(t.style.fg, Some(Color::Cyan));
        assert!(t.style.add_modifier.contains(Modifier::BOLD));
        let t2 = lines[1].spans.iter().find(|s| s.content.as_ref() == "Sub").unwrap();
        assert_eq!(t2.style.fg, Some(Color::Cyan));
        assert!(t2.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn list_items_get_cyan_bullets_and_indent() {
        let lines = md_lines("- one\n- two\n  - nested\n1. numbered");
        assert_eq!(lines.len(), 4);
        let l0 = &lines[0];
        assert!(l0.spans.iter().any(|s| s.content.as_ref() == "- "
            && s.style.fg == Some(Color::Cyan)));
        assert_eq!(flat_text(&lines[0..1]), "- one".to_string() + "\n".trim_end());
        let nested = &lines[2];
        assert!(
            nested.spans.first().map(|s| s.content.as_ref().contains(' ')).unwrap_or(false),
            "nested item is indented: {nested:?}"
        );
        let numbered = &lines[3];
        assert!(numbered.spans.iter().any(|s| s.content.as_ref() == "1. "));
    }

    #[test]
    fn fenced_code_block_has_dim_label_and_green_lines() {
        let lines = md_lines("```rust\nfn main() {}\n```");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(spans_of(&lines, 0)[0].content.as_ref(), "rust");
        assert_eq!(spans_of(&lines, 0)[0].style.fg, Some(Color::DarkGray));
        assert_eq!(spans_of(&lines, 1)[0].content.as_ref(), "fn main() {}");
        assert_eq!(spans_of(&lines, 1)[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn unclosed_fence_degrades_to_code_until_end() {
        // Streaming-safe: an open fence renders the rest as code, never
        // crashes and never swallows the label.
        let lines = md_lines("```py\nprint(1)");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(spans_of(&lines, 1)[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn inline_code_is_yellow() {
        let lines = md_lines("use `cargo build` now");
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        let code = s.iter().find(|s| s.content.as_ref() == "`cargo build`").unwrap();
        assert_eq!(code.style.fg, Some(Color::Yellow));
    }

    #[test]
    fn bold_and_italic() {
        let lines = md_lines("a **b** c *d* e");
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
        let text = flat_text(&md_lines("keep ~~f~~ intact"));
        assert!(text.contains("~~f~~"), "{text}");
    }

    #[test]
    fn links_render_text_blue_and_url_dim() {
        let lines = md_lines("see [docs](https://example.com/x) here");
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        let t = s.iter().find(|s| s.content.as_ref() == "docs").unwrap();
        assert_eq!(t.style.fg, Some(Color::Blue));
        let u = s.iter().find(|s| s.content.as_ref() == " (https://example.com/x)").unwrap();
        assert_eq!(u.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn blockquote_is_italic_with_dim_border() {
        let lines = md_lines("> quoted words");
        assert_eq!(lines.len(), 1);
        let s = &lines[0].spans;
        assert_eq!(s[0].content.as_ref(), "> ");
        assert_eq!(s[0].style.fg, Some(Color::DarkGray));
        let q = s.iter().find(|s| s.content.as_ref() == "quoted words").unwrap();
        assert!(q.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn unmatched_markup_stays_literal() {
        let lines = md_lines("2 * 3 = 6 and **unmatched");
        let text = flat_text(&lines);
        assert!(text.contains("2 * 3 = 6"), "{text}");
        assert!(text.contains("**unmatched"), "{text}");
    }

    #[test]
    fn mixed_document_snapshot() {
        let md = "# Plan\n- do `x` **now**\n  - detail\n```sh\necho hi\n```\n> note";
        let text = flat_text(&md_lines(md));
        assert_eq!(
            text,
            "1 Plan\n- do `x` now\n  - detail\nsh\necho hi\n> note",
            "layout: {text}"
        );
    }

    #[test]
    fn cjk_and_emoji_width_in_buffer() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Widget};
        // 你好 = 2 wide chars (4 cells), 👍 = 2 cells.
        let lines = md_lines("你好 👍 world");
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        Paragraph::new(lines).render(Rect::new(0, 0, 20, 1), &mut buf);
        let row: String = buf
            .content
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        // 你好 (4 cells) + space + 👍 (2 cells) + space + world (5) = 13 used.
        // Wide chars leave a blank continuation cell in the buffer, so the
        // raw row reads "你 好  👍  world".
        let world_col = row.chars().position(|c| c == 'w').unwrap();
        assert_eq!(world_col, 8, "emoji takes 2 cells, CJK 2 each: {row:?}");
        assert!(row.starts_with('你'), "{row:?}");
    }
}
