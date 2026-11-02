//! In-process fuzzy picker: filters on every keystroke without leaving the process.

use crate::config::{settings, RESET};
use crate::render::{Row, SEPARATOR};
use crossterm::cursor::{MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use std::io::{self, Write};

const PAGE: isize = 10;
const POINTER: &str = "▌ ";
const NO_POINTER: &str = "  ";
const PROMPT: &str = "› ";

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Continue,
    QueryChanged,
    Submit,
    Cancel,
}

pub struct State {
    pub query: String,
    rows: Vec<Row>,
    selected: usize,
    offset: usize,
    total: usize,
}

fn is_selectable(row: &Row) -> bool {
    row.0 != SEPARATOR
}

fn first_selectable(rows: &[Row]) -> usize {
    rows.iter().position(is_selectable).unwrap_or(0)
}

impl State {
    /// Opens on `focus_id` when it is in the list, otherwise on the first real row.
    pub fn new(rows: Vec<Row>, focus_id: Option<&str>) -> Self {
        let selected = focus_id
            .and_then(|id| rows.iter().position(|(row_id, _)| row_id == id))
            .unwrap_or_else(|| first_selectable(&rows));
        let total = rows.iter().filter(|r| is_selectable(r)).count();
        State { query: String::new(), rows, selected, offset: 0, total }
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.rows.get(self.selected).filter(|r| is_selectable(r)).map(|r| r.0.as_str())
    }

    pub fn match_count(&self) -> usize {
        self.rows.iter().filter(|r| is_selectable(r)).count()
    }

    /// New rows after the query changed: the first match becomes the selection.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.selected = first_selectable(&rows);
        self.offset = 0;
        self.rows = rows;
    }

    /// New rows after a resize: stay on the same row when it survives.
    pub fn refresh_rows(&mut self, rows: Vec<Row>) {
        let keep = self.selected_id().map(String::from);
        self.set_rows(rows);
        if let Some(index) = keep.and_then(|id| self.rows.iter().position(|(r, _)| *r == id)) {
            self.selected = index;
        }
    }

    fn move_by(&mut self, delta: isize) {
        let step = delta.signum();
        for _ in 0..delta.abs() {
            let mut next = self.selected as isize + step;
            while next >= 0 && (next as usize) < self.rows.len() && !is_selectable(&self.rows[next as usize]) {
                next += step;
            }
            if next < 0 || next as usize >= self.rows.len() {
                break;
            }
            self.selected = next as usize;
        }
    }

    pub fn apply(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Char('c' | 'g') if ctrl => Outcome::Cancel,
            KeyCode::Enter if self.selected_id().is_some() => Outcome::Submit,
            KeyCode::Down => self.moved(1),
            KeyCode::Char('n' | 'j') if ctrl => self.moved(1),
            KeyCode::Up => self.moved(-1),
            KeyCode::Char('p' | 'k') if ctrl => self.moved(-1),
            KeyCode::PageDown => self.moved(PAGE),
            KeyCode::PageUp => self.moved(-PAGE),
            KeyCode::Backspace => Self::edited(self.query.pop().is_some()),
            KeyCode::Char('u') if ctrl => Self::edited(!std::mem::take(&mut self.query).is_empty()),
            KeyCode::Char('w') if ctrl => {
                let kept = self.query.trim_end().rfind(char::is_whitespace).map_or(0, |i| i + 1);
                let changed = kept < self.query.len();
                self.query.truncate(kept);
                Self::edited(changed)
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                Outcome::QueryChanged
            }
            _ => Outcome::Continue,
        }
    }

    fn moved(&mut self, delta: isize) -> Outcome {
        self.move_by(delta);
        Outcome::Continue
    }

    fn edited(changed: bool) -> Outcome {
        if changed { Outcome::QueryChanged } else { Outcome::Continue }
    }

    /// Group headings directly above a row; they scroll into view with the group's first row.
    fn headings_above(&self, index: usize) -> usize {
        self.rows[..index.min(self.rows.len())].iter().rev().take_while(|r| !is_selectable(r)).count()
    }

    /// The slice of rows to draw, scrolling just enough to keep the selection in view.
    pub fn window(&mut self, height: usize) -> std::ops::Range<usize> {
        let top = self.selected - self.headings_above(self.selected);
        if top < self.offset {
            self.offset = top;
        }
        if height > 0 && self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
        self.offset..(self.offset + height).min(self.rows.len())
    }
}

/// Visible width of `text`, ignoring SGR escape sequences.
fn visible_len(text: &str) -> usize {
    let mut count = 0;
    walk_visible(text, |_, visible| count += usize::from(visible));
    count
}

/// Calls `f(char, counts_toward_width)` for each char, skipping over SGR sequences.
fn walk_visible(text: &str, mut f: impl FnMut(char, bool)) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';') {
                j += 1;
            }
            if chars.get(j) == Some(&'m') {
                chars[i..=j].iter().for_each(|c| f(*c, false));
                i = j + 1;
                continue;
            }
        }
        f(chars[i], true);
        i += 1;
    }
}

/// Cut to `width` visible columns, keeping escapes intact and resetting colors if cut.
pub fn truncate_visible(text: &str, width: usize) -> String {
    let (mut out, mut seen, mut cut) = (String::new(), 0, false);
    walk_visible(text, |c, visible| {
        if cut {
            return;
        }
        if visible && seen == width {
            cut = true;
        } else {
            out.push(c);
            seen += usize::from(visible);
        }
    });
    if cut {
        out.push_str(RESET);
    }
    out
}

fn selected_line(display: &str, width: usize, highlight: &str, key: &str) -> String {
    let body = truncate_visible(display, width.saturating_sub(POINTER.chars().count()));
    // A reset mid-row would end the highlight, so restart it after each one.
    let body = body.replace(RESET, &format!("{RESET}{highlight}"));
    let pad = width.saturating_sub(POINTER.chars().count() + visible_len(&body));
    format!("{highlight}{key}{POINTER}{RESET}{highlight}{body}{}{RESET}", " ".repeat(pad))
}

fn plain_line(display: &str, width: usize) -> String {
    format!("{NO_POINTER}{}", truncate_visible(display, width.saturating_sub(NO_POINTER.len())))
}

fn draw(out: &mut impl Write, state: &mut State, header: &str, width: usize, height: usize) -> io::Result<()> {
    let colors = &settings().colors;
    let list_height = height.saturating_sub(1);
    let window = state.window(list_height);
    let selected = state.selected;

    let mut count = format!("{}/{}", state.match_count(), state.total);
    if !header.is_empty() {
        count = format!("{count}  {header}");
    }
    let prompt = format!("{}{PROMPT}{RESET}{}", colors.key, state.query);
    let prompt = format!(
        "{prompt}  {}{}{RESET}",
        colors.dim,
        truncate_visible(&count, width.saturating_sub(visible_len(&prompt) + 2))
    );
    queue!(out, MoveTo(0, 0), crossterm::style::Print(truncate_visible(&prompt, width)))?;
    queue!(out, terminal::Clear(terminal::ClearType::UntilNewLine))?;

    for (line, index) in window.clone().enumerate() {
        let display = &state.rows[index].1;
        let text = if index == selected {
            selected_line(display, width, &settings().highlight, &colors.key)
        } else {
            plain_line(display, width)
        };
        queue!(out, MoveTo(0, line as u16 + 1), crossterm::style::Print(text))?;
        queue!(out, terminal::Clear(terminal::ClearType::UntilNewLine))?;
    }
    for line in window.len()..list_height {
        queue!(out, MoveTo(0, line as u16 + 1), terminal::Clear(terminal::ClearType::UntilNewLine))?;
    }
    let cursor_x = PROMPT.chars().count() + state.query.chars().count();
    queue!(out, MoveTo(cursor_x.min(width.saturating_sub(1)) as u16, 0))?;
    out.flush()
}

/// Puts the terminal in raw mode on an alternate screen, and restores it on drop.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let guard = TerminalGuard;
        execute!(io::stdout(), EnterAlternateScreen, Show)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn size() -> (usize, usize) {
    terminal::size().map_or((100, 24), |(w, h)| (w as usize, h as usize))
}

/// Runs the picker. `rows_for(query, width)` returns the rows to show for a query;
/// the picker returns the chosen row's id, or `None` when cancelled.
pub fn run(
    header: &str,
    focus_id: Option<&str>,
    mut rows_for: impl FnMut(&str, usize) -> Vec<Row>,
) -> Option<String> {
    let (width, _) = size();
    let mut state = State::new(rows_for("", width), focus_id);
    let _guard = TerminalGuard::enter().ok()?;
    let mut out = io::BufWriter::new(io::stdout());
    loop {
        let (width, height) = size();
        draw(&mut out, &mut state, header, width, height).ok()?;
        match event::read().ok()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match state.apply(key) {
                Outcome::Continue => {}
                Outcome::QueryChanged => {
                    let rows = rows_for(&state.query, width);
                    state.set_rows(rows);
                }
                Outcome::Submit => return state.selected_id().map(String::from),
                Outcome::Cancel => return None,
            },
            Event::Resize(..) => {
                let (width, _) = size();
                let rows = rows_for(&state.query, width);
                state.refresh_rows(rows);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn rows(ids: &[&str]) -> Vec<Row> {
        ids.iter().map(|id| (id.to_string(), format!("row {id}"))).collect()
    }

    /// Two groups, each opening with a blank row and a heading, as render_rows emits.
    fn grouped() -> Vec<Row> {
        let sep = || (SEPARATOR.to_string(), String::new());
        vec![sep(), sep(), ("a".into(), "A".into()), ("b".into(), "B".into()), sep(), sep(), ("c".into(), "C".into())]
    }

    #[test]
    fn opens_on_first_selectable_row_skipping_headings() {
        let state = State::new(grouped(), None);
        assert_eq!(state.selected_id(), Some("a"));
        assert_eq!(state.match_count(), 3);
    }

    #[test]
    fn opens_on_focus_row_when_present() {
        let state = State::new(grouped(), Some("c"));
        assert_eq!(state.selected_id(), Some("c"));
        let state = State::new(grouped(), Some("missing"));
        assert_eq!(state.selected_id(), Some("a"));
    }

    #[test]
    fn arrows_skip_group_headings_and_stop_at_the_ends() {
        let mut state = State::new(grouped(), None);
        state.apply(press(KeyCode::Down));
        assert_eq!(state.selected_id(), Some("b"));
        state.apply(press(KeyCode::Down));
        assert_eq!(state.selected_id(), Some("c"));
        state.apply(press(KeyCode::Down));
        assert_eq!(state.selected_id(), Some("c"));
        state.apply(press(KeyCode::Up));
        assert_eq!(state.selected_id(), Some("b"));
        state.apply(press(KeyCode::Up));
        state.apply(press(KeyCode::Up));
        assert_eq!(state.selected_id(), Some("a"));
    }

    #[test]
    fn emacs_style_keys_move_too() {
        let mut state = State::new(rows(&["a", "b", "c"]), None);
        state.apply(ctrl('n'));
        state.apply(ctrl('j'));
        assert_eq!(state.selected_id(), Some("c"));
        state.apply(ctrl('p'));
        state.apply(ctrl('k'));
        assert_eq!(state.selected_id(), Some("a"));
    }

    #[test]
    fn page_keys_jump_and_clamp() {
        let ids: Vec<String> = (0..30).map(|i| format!("r{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut state = State::new(rows(&refs), None);
        state.apply(press(KeyCode::PageDown));
        assert_eq!(state.selected_id(), Some("r10"));
        state.apply(press(KeyCode::PageUp));
        state.apply(press(KeyCode::PageUp));
        assert_eq!(state.selected_id(), Some("r0"));
    }

    #[test]
    fn typing_edits_the_query_and_reports_the_change() {
        let mut state = State::new(rows(&["a"]), None);
        assert_eq!(state.apply(press(KeyCode::Char('h'))), Outcome::QueryChanged);
        assert_eq!(state.apply(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)), Outcome::QueryChanged);
        assert_eq!(state.query, "hI");
        assert_eq!(state.apply(press(KeyCode::Backspace)), Outcome::QueryChanged);
        assert_eq!(state.query, "h");
    }

    #[test]
    fn backspace_on_empty_query_is_a_no_op() {
        let mut state = State::new(rows(&["a"]), None);
        assert_eq!(state.apply(press(KeyCode::Backspace)), Outcome::Continue);
    }

    #[test]
    fn ctrl_u_clears_and_ctrl_w_deletes_a_word() {
        let mut state = State::new(rows(&["a"]), None);
        state.query = "split right".into();
        assert_eq!(state.apply(ctrl('w')), Outcome::QueryChanged);
        assert_eq!(state.query, "split ");
        assert_eq!(state.apply(ctrl('u')), Outcome::QueryChanged);
        assert_eq!(state.query, "");
        assert_eq!(state.apply(ctrl('u')), Outcome::Continue);
        assert_eq!(state.apply(ctrl('w')), Outcome::Continue);
    }

    #[test]
    fn control_chars_are_not_typed_into_the_query() {
        let mut state = State::new(rows(&["a"]), None);
        assert_eq!(state.apply(ctrl('x')), Outcome::Continue);
        assert_eq!(state.query, "");
    }

    #[test]
    fn enter_submits_a_row_and_escape_cancels() {
        let mut state = State::new(rows(&["a"]), None);
        assert_eq!(state.apply(press(KeyCode::Enter)), Outcome::Submit);
        assert_eq!(state.apply(press(KeyCode::Esc)), Outcome::Cancel);
        assert_eq!(state.apply(ctrl('c')), Outcome::Cancel);
        assert_eq!(state.apply(ctrl('g')), Outcome::Cancel);
    }

    #[test]
    fn enter_with_no_rows_does_nothing() {
        let mut state = State::new(Vec::new(), None);
        assert_eq!(state.apply(press(KeyCode::Enter)), Outcome::Continue);
        assert_eq!(state.selected_id(), None);
    }

    #[test]
    fn new_rows_select_the_first_match() {
        let mut state = State::new(rows(&["a", "b"]), None);
        state.apply(press(KeyCode::Down));
        state.set_rows(rows(&["x", "y"]));
        assert_eq!(state.selected_id(), Some("x"));
    }

    #[test]
    fn refresh_keeps_the_selected_row() {
        let mut state = State::new(rows(&["a", "b", "c"]), None);
        state.apply(press(KeyCode::Down));
        state.refresh_rows(rows(&["a", "b", "c"]));
        assert_eq!(state.selected_id(), Some("b"));
        state.refresh_rows(rows(&["x"]));
        assert_eq!(state.selected_id(), Some("x"));
    }

    #[test]
    fn window_follows_the_selection_down_and_back_up() {
        let ids: Vec<String> = (0..20).map(|i| format!("r{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut state = State::new(rows(&refs), None);
        assert_eq!(state.window(5), 0..5);
        state.apply(press(KeyCode::PageDown));
        assert_eq!(state.window(5), 6..11);
        for _ in 0..10 {
            state.apply(press(KeyCode::Up));
        }
        assert_eq!(state.window(5), 0..5);
    }

    #[test]
    fn window_brings_group_headings_into_view_with_their_first_row() {
        let mut rows = grouped();
        rows.extend(grouped());
        let mut state = State::new(rows, None);
        state.apply(press(KeyCode::PageDown));
        // Selection is on the last row; scrolling back to the first row of a
        // group must reveal the two heading rows above it.
        state.window(3);
        state.selected = 2;
        assert_eq!(state.window(3).start, 0);
    }

    #[test]
    fn truncate_counts_visible_columns_only() {
        assert_eq!(truncate_visible("abcdef", 3), format!("abc{RESET}"));
        assert_eq!(truncate_visible("abc", 3), "abc");
        let colored = "\x1b[31mabcdef\x1b[0m";
        assert_eq!(truncate_visible(colored, 3), format!("\x1b[31mabc{RESET}"));
        assert_eq!(truncate_visible("\x1b[31mab\x1b[0m", 5), "\x1b[31mab\x1b[0m");
    }

    #[test]
    fn visible_len_ignores_escapes() {
        assert_eq!(visible_len("\x1b[38;2;1;2;3mabc\x1b[0m"), 3);
        assert_eq!(visible_len("a…b"), 3);
    }

    #[test]
    fn selected_line_fills_the_row_and_restarts_highlight_after_resets() {
        let line = selected_line("a\x1b[0mb", 10, "\x1b[7m", "\x1b[35m");
        assert!(line.starts_with("\x1b[7m\x1b[35m▌ "));
        assert!(line.contains(&format!("a{RESET}\x1b[7mb")));
        assert_eq!(visible_len(&line), 10);
    }
}
