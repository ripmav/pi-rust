//! WP-609: a generic list picker for the TUI (session resume + model
//! switch). Upstream semantics in v0: prefix filter (fuzzy search is out
//! of scope), `tui.select.*` keys (up/down/page wrap, Enter confirms the
//! selection, Esc/ctrl+c cancel), the selected row in the accent role.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::keybindings::KeybindingManager;

/// One selectable row: what the user sees, what the caller gets back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
	pub display: String,
	pub value: String,
}

/// The outcome of a closed picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
	Selected { display: String, value: String },
	Cancelled,
}

pub const MAX_VISIBLE: usize = 8;

#[derive(Debug, Clone)]
pub struct Picker {
	pub title: String,
	pub items: Vec<PickerItem>,
	/// The typed prefix filter.
	pub filter: String,
	/// Selection index into `visible()`.
	pub sel: usize,
}

impl Picker {
	pub fn new(title: impl Into<String>, items: Vec<PickerItem>) -> Self {
		Self { title: title.into(), items, filter: String::new(), sel: 0 }
	}

	/// Prefix-filtered items (out of scope: fuzzy search beyond prefix).
	pub fn visible(&self) -> Vec<PickerItem> {
		if self.filter.is_empty() {
			return self.items.clone();
		}
		self.items
			.iter()
			.filter(|i| i.display.to_lowercase().starts_with(&self.filter.to_lowercase()))
			.cloned()
			.collect()
	}

	pub fn is_empty(&self) -> bool {
		self.items.is_empty()
	}

	/// Handle one key. Returns the outcome when the picker closes.
	pub fn handle_key(&mut self, k: &KeyEvent, kb: &KeybindingManager) -> Option<PickerOutcome> {
		// The filter owns plain characters and backspace.
		if k.code == KeyCode::Backspace && k.modifiers == KeyModifiers::NONE {
			self.filter.pop();
			self.sel = 0;
			return None;
		}
		if let KeyCode::Char(c) = k.code {
			if !k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT) {
				self.filter.push(c);
				self.sel = 0;
				return None;
			}
		}
		if kb.matches(k, "tui.select.up") {
			let n = self.visible().len();
			if n > 0 {
				self.sel = if self.sel == 0 { n - 1 } else { self.sel - 1 };
			}
			return None;
		}
		if kb.matches(k, "tui.select.down") {
			let n = self.visible().len();
			if n > 0 {
				self.sel = (self.sel + 1) % n;
			}
			return None;
		}
		if kb.matches(k, "tui.select.pageUp") {
			let n = self.visible().len();
			if n > 0 {
				self.sel = self.sel.saturating_sub(MAX_VISIBLE);
			}
			return None;
		}
		if kb.matches(k, "tui.select.pageDown") {
			let n = self.visible().len();
			if n > 0 {
				self.sel = (self.sel + MAX_VISIBLE).min(n - 1);
			}
			return None;
		}
		if kb.matches(k, "tui.select.confirm") {
			let v = self.visible();
			return match v.get(self.sel.min(v.len().saturating_sub(1))) {
				Some(i) => Some(PickerOutcome::Selected { display: i.display.clone(), value: i.value.clone() }),
				None => Some(PickerOutcome::Cancelled),
			};
		}
		if kb.matches(k, "tui.select.cancel") {
			return Some(PickerOutcome::Cancelled);
		}
		None
	}

	/// Render rows (title + up to `MAX_VISIBLE` rows; an empty list renders
	/// a graceful placeholder). The caller provides the theme styles.
	pub fn render_rows(&self, selected: Style, rest: Style, muted: Style) -> Vec<Line<'static>> {
		let mut out = vec![Line::from(vec![Span::styled(
			format!("{} ({})", self.title, self.filter),
			muted,
		)])];
		let v = self.visible();
		if v.is_empty() {
			out.push(Line::from(Span::styled("  (no matches)", muted)));
			return out;
		}
		for (i, item) in v.iter().take(MAX_VISIBLE).enumerate() {
			let style = if i == self.sel { selected } else { rest };
			let marker = if i == self.sel { "▸ " } else { "  " };
			out.push(Line::from(Span::styled(format!("{marker}{}/{}", self.sel_offset(i), item.display), style)));
		}
		out
	}

	fn sel_offset(&self, visible_idx: usize) -> usize {
		// The row number in the full (unfiltered) list, for stable numbering.
		let full: Vec<&PickerItem> = self.items.iter().collect();
		let start = if self.filter.is_empty() {
			0
		} else {
			full.iter().position(|i| i.display.to_lowercase().starts_with(&self.filter.to_lowercase())).unwrap_or(0)
		};
		start + visible_idx
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn kb() -> KeybindingManager {
		KeybindingManager::default()
	}
	fn key(code: KeyCode) -> KeyEvent {
		KeyEvent::new(code, KeyModifiers::NONE)
	}
	fn items(n: usize) -> Vec<PickerItem> {
		(0..n).map(|i| PickerItem { display: format!("item-{i:02}"), value: format!("v{i:02}") }).collect()
	}

	#[test]
	fn filter_is_prefix_case_insensitive() {
		let mut p = Picker::new("t", items(30));
		p.handle_key(&key(KeyCode::Char('I')), &kb());
		p.handle_key(&key(KeyCode::Char('T')), &kb());
		assert_eq!(p.visible().len(), 30);
		p.handle_key(&key(KeyCode::Char('5')), &kb());
		let v = p.visible();
		assert!(v.iter().all(|i| i.display.starts_with("it-5")), "{v:?}");
		// Backspace removes the last filter char.
		p.handle_key(&key(KeyCode::Backspace), &kb());
		assert_eq!(p.visible().len(), 30);
	}

	#[test]
	fn navigation_wraps_and_pages() {
		let mut p = Picker::new("t", items(12));
		p.handle_key(&key(KeyCode::Up), &kb());
		assert_eq!(p.sel, 11, "up on 0 wraps to the last row");
		p.handle_key(&key(KeyCode::Down), &kb());
		assert_eq!(p.sel, 0);
		p.handle_key(&key(KeyCode::PageDown), &kb());
		assert_eq!(p.sel, 8);
		p.handle_key(&key(KeyCode::PageDown), &kb());
		assert_eq!(p.sel, 11, "page down clamps at the end");
		p.handle_key(&key(KeyCode::PageUp), &kb());
		assert_eq!(p.sel, 3);
	}

	#[test]
	fn enter_selects_and_esc_cancels() {
		let mut p = Picker::new("t", items(5));
		p.handle_key(&key(KeyCode::Down), &kb());
		match p.handle_key(&key(KeyCode::Enter), &kb()) {
			Some(PickerOutcome::Selected { display, value }) => {
				assert_eq!((display, value), ("item-01".to_string(), "v01".to_string()));
			}
			other => panic!("expected selection, got {other:?}"),
		}
		let mut p = Picker::new("t", items(5));
		assert_eq!(p.handle_key(&key(KeyCode::Esc), &kb()), Some(PickerOutcome::Cancelled));
	}

	#[test]
	fn empty_list_is_graceful() {
		let p = Picker::new("empty", vec![]);
		assert!(p.is_empty());
		let rows = p.render_rows(Style::default(), Style::default(), Style::default());
		assert!(rows.iter().any(|l| l.to_string().contains("no matches")), "{rows:?}");
		// Enter on an empty picker cancels (nothing to select).
		let mut p = Picker::new("empty", vec![]);
		assert_eq!(p.handle_key(&key(KeyCode::Enter), &kb()), Some(PickerOutcome::Cancelled));
	}

	#[test]
	fn render_rows_number_and_mark_the_selection() {
		let mut p = Picker::new("Resume", items(3));
		p.handle_key(&key(KeyCode::Down), &kb());
		let rows = p.render_rows(Style::default(), Style::default(), Style::default());
		let text: Vec<String> = rows.iter().map(|l| l.to_string()).collect();
		assert_eq!(text[0], "Resume ()");
		assert!(text[1].starts_with("  0/"), "{text:?}");
		assert!(text[2].starts_with("▸ 1/"), "{text:?}");
		assert!(text[3].starts_with("  2/"), "{text:?}");
	}
}
