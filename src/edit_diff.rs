//! Port of the pinned upstream `packages/coding-agent/src/core/tools/edit-diff.ts`
//! (plus `splitBom` from `src/utils/text.ts`) backing the built-in `edit` tool.
//!
//! Pinned deviations (WP-503):
//! - Fuzzy matching (`fuzzyFindText` / `normalizeForFuzzyMatch`) is out of scope:
//!   matching is exact only. The no-match / ambiguous error strings are upstream's.
//! - `generate_edit_diff` diffs the whole content instead of per-replacement
//!   windows (`getDiffWindows`). Upstream's own property pins the windowed output
//!   to the whole-file output for distinct lines; windowing is a memory
//!   optimization for huge files, and the TUI renderer that would consume it is
//!   not part of v0.
//! - The line diff is an LCS dynamic program instead of jsdiff's Myers walk
//!   (`diff` crate is not available; no new dependencies). For distinct lines —
//!   what the edit tool produces — both give the same parts; jsdiff itself only
//!   differs on runs of repeated lines (upstream documents the same caveat).
//!   shortcut: O(lines²) memory, upgrade to Myers if editing files beyond ~5k lines.

/// Upstream `normalizeToLF`: CRLF → LF, then lone CR → LF.
pub fn normalize_to_lf(text: &str) -> String {
	text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Upstream `splitBom`: split a leading UTF-8 byte order mark from decoded text.
pub fn split_bom(content: &str) -> (&str, &str) {
	content
	.strip_prefix('\u{FEFF}')
	.map(|rest| ("\u{FEFF}", rest))
	.unwrap_or(("", content))
}

#[derive(Debug)]
pub struct TextReplacement {
	pub match_index: usize,
	pub match_length: usize,
	pub new_text: String,
}

#[derive(Debug)]
pub struct AppliedEdits {
	pub base_content: String,
	pub new_content: String,
	pub replacements: Vec<TextReplacement>,
}

#[derive(Debug)]
struct MatchedEdit {
	edit_index: usize,
	match_index: usize,
	match_length: usize,
	new_text: String,
}

/// Upstream `applyEditsToNormalizedContent`, exact matching only: all edits match the
/// same original (normalized) content, never incrementally. Replacements are applied in
/// reverse order so offsets stay stable. Returns upstream's exact error strings.
pub fn apply_edits_to_normalized_content(
	normalized_content: &str,
	edits: &[(String, String)],
	path: &str,
) -> Result<AppliedEdits, String> {
	let total = edits.len();
	let normalized_edits: Vec<(String, String)> = edits
		.iter()
		.map(|(old_text, new_text)| (normalize_to_lf(old_text), normalize_to_lf(new_text)))
		.collect();
	for (i, (old_text, _)) in normalized_edits.iter().enumerate() {
		if old_text.is_empty() {
			return Err(if total == 1 {
				format!("oldText must not be empty in {path}.")
			} else {
				format!("edits[{i}].oldText must not be empty in {path}.")
			});
		}
	}
	let mut matched: Vec<MatchedEdit> = Vec::with_capacity(normalized_edits.len());
	for (i, (old_text, new_text)) in normalized_edits.iter().enumerate() {
		let Some(match_index) = normalized_content.find(old_text.as_str()) else {
			return Err(if total == 1 {
				format!(
					"Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
				)
			} else {
				format!(
					"Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines."
				)
			});
		};
		let occurrences = normalized_content.matches(old_text.as_str()).count();
		if occurrences > 1 {
			return Err(if total == 1 {
				format!(
					"Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
				)
			} else {
				format!(
					"Found {occurrences} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
				)
			});
		}
		matched.push(MatchedEdit {
			edit_index: i,
			match_index,
			match_length: old_text.len(),
			new_text: new_text.clone(),
		});
	}
	matched.sort_by_key(|m| m.match_index);
	for pair in matched.windows(2) {
		let (previous, current) = (&pair[0], &pair[1]);
		if previous.match_index + previous.match_length > current.match_index {
			return Err(format!(
				"edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
				previous.edit_index, current.edit_index
			));
		}
	}
	let base_content = normalized_content.to_string();
	let mut new_content = base_content.clone();
	for m in matched.iter().rev() {
		let (head, tail) = new_content.split_at(m.match_index);
		let tail = &tail[m.match_length..];
		new_content = format!("{head}{}{tail}", m.new_text);
	}
	if base_content == new_content {
		return Err(if total == 1 {
			format!(
				"No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
			)
		} else {
			format!("No changes made to {path}. The replacements produced identical content.")
		});
	}
	Ok(AppliedEdits {
		base_content,
		new_content,
		replacements: matched
			.into_iter()
			.map(|m| TextReplacement {
				match_index: m.match_index,
				match_length: m.match_length,
				new_text: m.new_text,
			})
			.collect(),
	})
}

/// Upstream `applyReplacementsToOriginal`: splice replacements (matched in the
/// LF-normalized view) back into the original content so untouched lines keep their
/// bytes, including line endings. Inserted line breaks take the ending of the line
/// where the replaced range starts.
pub fn apply_replacements_to_original(original_content: &str, replacements: &[TextReplacement]) -> String {
	// Only CRLF changes length when normalizing: the LF of the n-th CRLF (at original offset p) is at
	// p - n in the normalized text. Adding the number of CRLFs before a normalized offset gives the
	// original offset, and a range that covers that LF covers the whole CRLF.
	let mut crlf_before = 0usize;
	let mut next_crlf = original_content.find("\r\n");
	let mut to_original = |offset: usize| -> usize {
		while let Some(nc) = next_crlf {
			if nc - crlf_before < offset {
				crlf_before += 1;
				next_crlf = original_content[nc + 2..].find("\r\n").map(|p| p + nc + 2);
			} else {
				break;
			}
		}
		offset + crlf_before
	};

	let mut parts: Vec<String> = Vec::new();
	let mut ends_with_cr = false;
	let mut push = |part: &str, parts: &mut Vec<String>| {
		if part.is_empty() {
			return;
		}
		// A CR before a seam and an LF after it would merge into one CRLF line break, for example a kept
		// lone CR followed by inserted text that starts with LF, or both sides of a deleted range. Adding
		// an LF turns the CR into a CRLF of its own, so the written file has the line breaks the diff shows.
		if ends_with_cr && part.starts_with('\n') {
			parts.push("\n".to_string());
		}
		parts.push(part.to_string());
		ends_with_cr = part.ends_with('\r');
	};
	let mut position = 0usize;
	for r in replacements {
		let start = to_original(r.match_index);
		push(&original_content[position..start], &mut parts);
		let ending = get_line_ending_at(original_content, start);
		let new_text = if ending == "\n" {
			r.new_text.clone()
		} else {
			r.new_text.replace('\n', ending)
		};
		push(&new_text, &mut parts);
		position = to_original(r.match_index + r.match_length);
	}
	push(&original_content[position..], &mut parts);
	parts.join("")
}

/// Line ending of the line containing `offset`: its own LF or CRLF, or that of the line before
/// for an unterminated last line. A lone CR only counts in files without any LF (classic Mac
/// line endings); elsewhere it is more likely a control character inside a line.
fn get_line_ending_at(content: &str, offset: usize) -> &'static str {
	let mut line_feed = content[offset..].find('\n').map(|p| p + offset);
	if line_feed.is_none() {
		line_feed = content[..=offset].rfind('\n');
	}
	match line_feed {
		None => {
			if content.contains('\r') {
				"\r"
			} else {
				"\n"
			}
		}
		Some(lf) => {
			// `lf == 0` mirrors JS `content[-1] === undefined`: a line break at the very start of the file has no preceding CR.
			if lf > 0 && content.as_bytes()[lf - 1] == b'\r' {
				"\r\n"
			} else {
				"\n"
			}
		}
	}
}

#[derive(Debug)]
pub struct EditDiff {
	pub diff: String,
	pub patch: String,
	pub first_changed_line: Option<usize>,
}

/// Display diff + unified patch of an edit, as returned in the edit tool's `details`
/// (upstream `EditToolDetails`). Whole-file variant of upstream `generateEditDiff`;
/// the context and line-number format match jsdiff's output, which upstream wraps.
pub fn generate_edit_diff(path: &str, base_content: &str, new_content: &str) -> EditDiff {
	const CONTEXT: usize = 4;
	let line_num_width = count_lines(base_content)
		.max(count_lines(new_content))
		.to_string()
		.len();
	let parts = line_diff(base_content, new_content);
	let (diff_lines, first_changed_line) = format_display_diff(&parts, line_num_width, CONTEXT);
	let hunks = structured_patch(&parts, CONTEXT);
	EditDiff {
		diff: diff_lines.join("\n"),
		patch: format_patch(path, &hunks),
		first_changed_line,
	}
}

/// Number of lines as `text.split("\n").length` counts them, without building the array.
fn count_lines(text: &str) -> usize {
	text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// One part of the line diff: consecutive lines of the same kind, jsdiff-style values
/// (line content plus its line ending; the last line has none).
#[derive(Debug, Clone)]
struct Part {
	value: String,
	added: bool,
	removed: bool,
}

/// Upstream `Diff.diffLines` tokenization: content plus its line ending (`\n`, or `\r\n`
/// when the CR directly precedes it); a lone CR is line content. Trailing empty token dropped.
fn tokenize_lines(text: &str) -> Vec<String> {
	let mut tokens = Vec::new();
	let mut start = 0usize;
	for (i, b) in text.bytes().enumerate() {
		if b == b'\n' {
			tokens.push(text[start..=i].to_string());
			start = i + 1;
		}
	}
	tokens.push(text[start..].to_string());
	tokens.retain(|t| !t.is_empty());
	tokens
}

fn push_part(parts: &mut Vec<Part>, value: &str, added: bool, removed: bool) {
	match parts.last_mut() {
		Some(p) if p.added == added && p.removed == removed => p.value.push_str(value),
		_ => parts.push(Part {
			value: value.to_string(),
			added,
			removed,
		}),
	}
}

/// Line diff of `old` vs `new` as jsdiff `diffLines` parts (common/removed/added runs).
fn line_diff(old: &str, new: &str) -> Vec<Part> {
	let a = tokenize_lines(old);
	let b = tokenize_lines(new);
	let n = a.len();
	let m = b.len();
	// dp[i][j] = LCS length of a[i..] and b[j..].
	let mut dp = vec![vec![0u32; m + 1]; n + 1];
	for i in (0..n).rev() {
		for j in (0..m).rev() {
			dp[i][j] = if a[i] == b[j] {
				dp[i + 1][j + 1] + 1
			} else {
				dp[i + 1][j].max(dp[i][j + 1])
			};
		}
	}
	let mut parts = Vec::new();
	let (mut i, mut j) = (0usize, 0usize);
	while i < n && j < m {
		if a[i] == b[j] {
			push_part(&mut parts, &a[i], false, false);
			i += 1;
			j += 1;
		} else if dp[i + 1][j] >= dp[i][j + 1] {
			push_part(&mut parts, &a[i], false, true);
			i += 1;
		} else {
			push_part(&mut parts, &b[j], true, false);
			j += 1;
		}
	}
	while i < n {
		push_part(&mut parts, &a[i], false, true);
		i += 1;
	}
	while j < m {
		push_part(&mut parts, &b[j], true, false);
		j += 1;
	}
	parts
}

/// Upstream skip marker: one space + the line-number column + " ...".
fn skip_marker(output: &mut Vec<String>, line_num_width: usize) {
	output.push(format!("{} ...", " ".repeat(line_num_width + 1)));
}

/// Upstream `formatDisplayDiff`: numbered `+`/`-`/context lines, at most `context` context
/// lines around each change, `...` markers for skipped runs, first changed new-file line.
fn format_display_diff(
	parts: &[Part],
	line_num_width: usize,
	context: usize,
) -> (Vec<String>, Option<usize>) {
	let mut output: Vec<String> = Vec::new();
	let (mut old_line_num, mut new_line_num) = (1usize, 1usize);
	let mut last_was_change = false;
	let mut first_changed_line: Option<usize> = None;
	for (i, part) in parts.iter().enumerate() {
		let mut raw: Vec<&str> = part.value.split('\n').collect();
		if raw.last() == Some(&"") {
			raw.pop();
		}
		if part.added || part.removed {
			if first_changed_line.is_none() {
				first_changed_line = Some(new_line_num);
			}
			for line in &raw {
				if part.added {
					output.push(format!(
						"+{} {}",
						format!("{:width$}", new_line_num, width = line_num_width),
						line
					));
					new_line_num += 1;
				} else {
					output.push(format!(
						"-{} {}",
						format!("{:width$}", old_line_num, width = line_num_width),
						line
					));
					old_line_num += 1;
				}
			}
			last_was_change = true;
		} else {
			let next_part_is_change = i + 1 < parts.len() && (parts[i + 1].added || parts[i + 1].removed);
			let has_leading_change = last_was_change;
			let has_trailing_change = next_part_is_change;
			if has_leading_change && has_trailing_change {
				if raw.len() <= context * 2 {
					for line in &raw {
						output.push(format!(" {} {}", format!("{:width$}", old_line_num, width = line_num_width), line));
						old_line_num += 1;
						new_line_num += 1;
					}
				} else {
					let leading_lines = &raw[..context];
					let trailing_lines = &raw[raw.len() - context..];
					let skipped_lines = raw.len() - leading_lines.len() - trailing_lines.len();
					for line in leading_lines {
						output.push(format!(" {} {}", format!("{:width$}", old_line_num, width = line_num_width), line));
						old_line_num += 1;
						new_line_num += 1;
					}
					skip_marker(&mut output, line_num_width);
					old_line_num += skipped_lines;
					new_line_num += skipped_lines;
					for line in trailing_lines {
						output.push(format!(" {} {}", format!("{:width$}", old_line_num, width = line_num_width), line));
						old_line_num += 1;
						new_line_num += 1;
					}
				}
			} else if has_leading_change {
				let shown_lines = &raw[..raw.len().min(context)];
				let skipped_lines = raw.len() - shown_lines.len();
				for line in shown_lines {
					output.push(format!(" {} {}", format!("{:width$}", old_line_num, width = line_num_width), line));
					old_line_num += 1;
					new_line_num += 1;
				}
				if skipped_lines > 0 {
					skip_marker(&mut output, line_num_width);
					old_line_num += skipped_lines;
					new_line_num += skipped_lines;
				}
			} else if has_trailing_change {
				let skipped_lines = raw.len().saturating_sub(context);
				if skipped_lines > 0 {
					skip_marker(&mut output, line_num_width);
					old_line_num += skipped_lines;
					new_line_num += skipped_lines;
				}
				for line in &raw[skipped_lines..] {
					output.push(format!(" {} {}", format!("{:width$}", old_line_num, width = line_num_width), line));
					old_line_num += 1;
					new_line_num += 1;
				}
			} else {
				old_line_num += raw.len();
				new_line_num += raw.len();
			}
			last_was_change = false;
		}
	}
	(output, first_changed_line)
}

/// `text.split("\n")` with the trailing newline reattached to each line (jsdiff
/// `splitLines` in patch/create.js); the final line keeps no newline when `text` has none.
fn split_lines(text: &str) -> Vec<String> {
	let has_trailing_nl = text.ends_with('\n');
	let mut result: Vec<String> = text.split('\n').map(|line| format!("{line}\n")).collect();
	if has_trailing_nl {
		result.pop();
	} else {
		let last = result.pop().expect("split always yields at least one line");
		result.push(last[..last.len() - 1].to_string());
	}
	result
}

struct Hunk {
	old_start: usize,
	old_lines: usize,
	new_start: usize,
	new_lines: usize,
	lines: Vec<String>,
}

/// Upstream `structuredPatch` (jsdiff patch/create.js): unified hunks of at most
/// `context` lines of leading/trailing context, merging overlapping ranges.
fn structured_patch(parts: &[Part], context: usize) -> Vec<Hunk> {
	let mut diff: Vec<Part> = parts.to_vec();
	diff.push(Part {
		value: String::new(),
		added: false,
		removed: false,
	});
	let mut hunks: Vec<Hunk> = Vec::new();
	let (mut old_range_start, mut new_range_start) = (0usize, 0usize);
	let mut cur_range: Vec<String> = Vec::new();
	let (mut old_line, mut new_line) = (1usize, 1usize);
	for (i, current) in diff.iter().enumerate() {
		let lines = split_lines(&current.value);
		if current.added || current.removed {
			if old_range_start == 0 {
				old_range_start = old_line;
				new_range_start = new_line;
				if i > 0 && context > 0 {
					let prev_lines = split_lines(&diff[i - 1].value);
					let mut leading: Vec<String> = prev_lines
						.iter()
						.rev()
						.take(context)
						.map(|line| format!(" {line}"))
						.collect();
					leading.reverse();
					cur_range = leading;
					old_range_start = old_range_start.saturating_sub(cur_range.len());
					new_range_start = new_range_start.saturating_sub(cur_range.len());
				}
			}
			let prefix = if current.added { '+' } else { '-' };
			for line in &lines {
				cur_range.push(format!("{prefix}{line}"));
			}
			if current.added {
				new_line += lines.len();
			} else {
				old_line += lines.len();
			}
		} else if old_range_start != 0 {
			if lines.len() <= context * 2 && i < diff.len() - 2 {
				// Overlapping: keep the range open with the full context block.
				for line in &lines {
					cur_range.push(format!(" {line}"));
				}
			} else {
				let context_size = lines.len().min(context);
				for line in &lines[..context_size] {
					cur_range.push(format!(" {line}"));
				}
				hunks.push(Hunk {
					old_start: old_range_start,
					old_lines: old_line - old_range_start + context_size,
					new_start: new_range_start,
					new_lines: new_line - new_range_start + context_size,
					lines: std::mem::take(&mut cur_range),
				});
				old_range_start = 0;
				new_range_start = 0;
			}
		}
		if !current.added && !current.removed {
			old_line += lines.len();
			new_line += lines.len();
		}
	}
	// Step 2: drop the trailing `\n` from each hunk line; a line without one earns the
	// marker after it (jsdiff step 2).
	for hunk in &mut hunks {
		let mut cleaned: Vec<String> = Vec::with_capacity(hunk.lines.len() * 2);
		for line in &hunk.lines {
			match line.strip_suffix('\n') {
				Some(stripped) => cleaned.push(stripped.to_string()),
				None => {
					cleaned.push(line.clone());
					cleaned.push("\\ No newline at end of file".to_string());
				}
			}
		}
		hunk.lines = cleaned;
	}
	hunks
}

/// Upstream `formatPatch` with `FILE_HEADERS_ONLY`: `--- path` / `+++ path` plus the hunks.
fn format_patch(path: &str, hunks: &[Hunk]) -> String {
	let mut out = String::from(&format!("--- {path}\n+++ {path}\n"));
	for hunk in hunks {
		// Unified Diff Format quirk: if the chunk size is 0, the first number is one lower.
		let old_start = if hunk.old_lines == 0 { hunk.old_start.saturating_sub(1) } else { hunk.old_start };
		let new_start = if hunk.new_lines == 0 { hunk.new_start.saturating_sub(1) } else { hunk.new_start };
		out.push_str(&format!(
			"@@ -{old_start},{} +{new_start},{} @@\n",
			hunk.old_lines, hunk.new_lines
		));
		for line in &hunk.lines {
			out.push_str(line);
			out.push('\n');
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	const P: &str = "file.txt";

	#[test]
	fn normalize_to_lf_converts_crlf_and_lone_cr() {
		assert_eq!(normalize_to_lf("a\r\nb\rc\n"), "a\nb\nc\n");
		assert_eq!(normalize_to_lf("plain\n"), "plain\n");
	}

	#[test]
	fn split_bom_strips_leading_bom_only() {
		assert_eq!(split_bom("\u{FEFF}abc"), ("\u{FEFF}", "abc"));
		assert_eq!(split_bom("abc"), ("", "abc"));
		assert_eq!(split_bom("x\u{FEFF}"), ("", "x\u{FEFF}"));
	}

	#[test]
	fn apply_unique_single_edit() {
		let r = apply_edits_to_normalized_content("Hello, world!", &[("world".into(), "testing".into())], P).unwrap();
		assert_eq!(r.base_content, "Hello, world!");
		assert_eq!(r.new_content, "Hello, testing!");
		assert_eq!(r.replacements.len(), 1);
		assert_eq!(r.replacements[0].match_index, 7);
		assert_eq!(r.replacements[0].match_length, 5);
		assert_eq!(r.replacements[0].new_text, "testing");
	}

	#[test]
	fn apply_multi_edit_matched_against_original_not_incremental() {
		// Upstream: edits match the ORIGINAL content; the "bar\n" inside edit 0's newText
		// must not make edit 1 ambiguous.
		let r = apply_edits_to_normalized_content(
			"foo\nbar\nbaz\n",
			&[("foo\n".into(), "foo bar\n".into()), ("bar\n".into(), "BAR\n".into())],
			P,
		)
		.unwrap();
		assert_eq!(r.new_content, "foo bar\nBAR\nbaz\n");
	}

	#[test]
	fn apply_no_match_single_exact_error() {
		let err = apply_edits_to_normalized_content("Hello, world!", &[("nonexistent".into(), "x".into())], P)
			.unwrap_err();
		assert_eq!(
			err,
			"Could not find the exact text in file.txt. The old text must match exactly including all whitespace and newlines."
		);
	}

	#[test]
	fn apply_no_match_multi_exact_error() {
		let err = apply_edits_to_normalized_content(
			"alpha\nbeta\n",
			&[("alpha\n".into(), "A\n".into()), ("missing\n".into(), "M\n".into())],
			P,
		)
		.unwrap_err();
		assert_eq!(
			err,
			"Could not find edits[1] in file.txt. The oldText must match exactly including all whitespace and newlines."
		);
	}

	#[test]
	fn apply_ambiguous_single_exact_error() {
		let err = apply_edits_to_normalized_content("foo foo foo", &[("foo".into(), "bar".into())], P).unwrap_err();
		assert_eq!(
			err,
			"Found 3 occurrences of the text in file.txt. The text must be unique. Please provide more context to make it unique."
		);
	}

	#[test]
	fn apply_ambiguous_multi_exact_error() {
		let err =
			apply_edits_to_normalized_content("x\nx\ny\n", &[("x\n".into(), "A\n".into()), ("y\n".into(), "B\n".into())], P)
				.unwrap_err();
		assert_eq!(
			err,
			"Found 2 occurrences of edits[0] in file.txt. Each oldText must be unique. Please provide more context to make it unique."
		);
	}

	#[test]
	fn apply_empty_oldtext_exact_errors() {
		let err = apply_edits_to_normalized_content("a", &[(String::new(), "x".into())], P).unwrap_err();
		assert_eq!(err, "oldText must not be empty in file.txt.");
		let err = apply_edits_to_normalized_content("a\nb", &[("a\n".into(), "x".into()), (String::new(), "y".into())], P)
			.unwrap_err();
		assert_eq!(err, "edits[1].oldText must not be empty in file.txt.");
	}

	#[test]
	fn apply_no_change_exact_errors() {
		let err = apply_edits_to_normalized_content("a", &[("a".into(), "a".into())], P).unwrap_err();
		assert_eq!(
			err,
			"No changes made to file.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
		);
		let err = apply_edits_to_normalized_content("a\nb", &[("a\n".into(), "a\n".into()), ("b".into(), "b".into())], P)
			.unwrap_err();
		assert_eq!(err, "No changes made to file.txt. The replacements produced identical content.");
	}

	#[test]
	fn apply_overlapping_edits_exact_error() {
		let err = apply_edits_to_normalized_content(
			"one\ntwo\nthree\n",
			&[("one\ntwo\n".into(), "ONE\nTWO\n".into()), ("two\nthree\n".into(), "TWO\nTHREE\n".into())],
			P,
		)
		.unwrap_err();
		assert_eq!(
			err,
			"edits[0] and edits[1] overlap in file.txt. Merge them into one edit or target disjoint regions."
		);
	}

	#[test]
	fn apply_replacements_to_original_preserves_crlf() {
		let repl = vec![TextReplacement { match_index: 6, match_length: 7, new_text: "REPLACED\n".into() }];
		assert_eq!(
			apply_replacements_to_original("first\r\nsecond\r\nthird\r\n", &repl),
			"first\r\nREPLACED\r\nthird\r\n"
		);
	}

	#[test]
	fn apply_replacements_to_original_mixed_endings_multi_edit() {
		// Upstream #8544: inserted lines take the line ending of the line they replace.
		// Original "lf1\nlf2\ncr1\r\ncr2\r\n", normalized "lf1\nlf2\ncr1\ncr2\n":
		// edit 0: "lf1\n"(0..4) -> "A\nB\n"; edit 1: "cr1\n"(8..12) -> "C\nD\n".
		let repl = vec![
			TextReplacement { match_index: 0, match_length: 4, new_text: "A\nB\n".into() },
			TextReplacement { match_index: 8, match_length: 4, new_text: "C\nD\n".into() },
		];
		assert_eq!(
			apply_replacements_to_original("lf1\nlf2\ncr1\r\ncr2\r\n", &repl),
			"A\nB\nlf2\nC\r\nD\r\ncr2\r\n"
		);
	}

	#[test]
	fn apply_replacements_to_original_lone_cr_seam_not_merged() {
		// Upstream: a kept CR and an LF meeting at a splice seam must not merge into one CRLF.
		let repl = vec![TextReplacement { match_index: 2, match_length: 1, new_text: "\nB".into() }];
		assert_eq!(apply_replacements_to_original("a\r\nb\nc", &repl), "a\r\n\nB\nc");
		let repl = vec![TextReplacement { match_index: 2, match_length: 1, new_text: String::new() }];
		assert_eq!(apply_replacements_to_original("a\r\nb\nc", &repl), "a\r\n\nc");
	}

	#[test]
	fn apply_replacements_to_original_lf_at_byte_zero() {
		// A line break at the very start of the file: JS `content[-1]` is undefined, so the
		// line ending is plain LF (Rust would underflow on `lf - 1` without the guard).
		let repl = vec![TextReplacement { match_index: 0, match_length: 4, new_text: "BAR\n".into() }];
		assert_eq!(apply_replacements_to_original("\nfoo\n", &repl), "BAR\n\n");
	}

	#[test]
	fn apply_replacements_to_original_lf_untouched() {
		let repl = vec![TextReplacement { match_index: 6, match_length: 7, new_text: "REPLACED\n".into() }];
		assert_eq!(
			apply_replacements_to_original("first\nsecond\nthird\n", &repl),
			"first\nREPLACED\nthird\n"
		);
	}

	#[test]
	fn generate_edit_diff_simple_exact_strings() {
		let d = generate_edit_diff(P, "Hello, world!", "Hello, testing!");
		assert_eq!(d.diff, "-1 Hello, world!\n+1 Hello, testing!");
		assert_eq!(d.first_changed_line, Some(1));
		assert_eq!(
			d.patch,
			"--- file.txt\n+++ file.txt\n@@ -1,2 +1,2 @@\n-Hello, world!\n\\ No newline at end of file\n+Hello, testing!\n\\ No newline at end of file\n \n\\ No newline at end of file\n"
		);
	}

	#[test]
	fn generate_edit_diff_collapses_large_gaps() {
		// Upstream "should collapse large unchanged gaps in multi-edit diffs".
		let lines: Vec<String> = (1..=600).map(|i| format!("line {:03}", i)).collect();
		let base = format!("{}\n", lines.join("\n"));
		let new = base
			.replace("line 100\n", "LINE 100\n")
			.replace("line 300\n", "LINE 300\n")
			.replace("line 500\n", "LINE 500\n");
		let d = generate_edit_diff(P, &base, &new);
		assert!(d.diff.contains("LINE 100") && d.diff.contains("LINE 300") && d.diff.contains("LINE 500"));
		assert!(d.diff.contains("..."));
		assert!(!d.diff.contains("line 250"));
		assert!(d.diff.lines().count() < 50);
		assert_eq!(d.first_changed_line, Some(100));
		// One hunk per change: 4 context lines + the changed line, starting 4 lines before.
		assert!(d.patch.contains("@@ -96,9 +96,9 @@"));
		assert!(d.patch.contains("@@ -296,9 +296,9 @@"));
		assert!(d.patch.contains("@@ -496,9 +496,9 @@"));
	}
}
