//! Application state and key handling.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use efivars_core::decode::{Kind, classify};
use efivars_core::load_option::{self, LOAD_OPTION_ACTIVE, LoadOption};
use efivars_core::store::DirStore;
use efivars_core::{VarKey, VarStore, Variable, attrs, guids};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Decoded,
    Hex,
}

pub struct HexEditor {
    pub data: Vec<u8>,
    pub cursor: usize,
    /// Next digit typed replaces the high nibble.
    pub high_nibble: bool,
}

pub struct OrderEditor {
    pub order: Vec<u16>,
    pub selected: usize,
}

pub struct PendingWrite {
    pub var: Variable,
    pub summary: Vec<String>,
}

pub enum Mode {
    Normal,
    Filter,
    HexEdit(HexEditor),
    OrderEdit(OrderEditor),
    Confirm(PendingWrite),
}

pub struct App {
    store: Box<dyn VarStore>,
    pub write_enabled: bool,
    pub vars: Vec<Variable>,
    /// Indices into `vars` matching the filter.
    pub filtered: Vec<usize>,
    pub list_state: ListState,
    pub filter: String,
    pub view: View,
    pub detail_scroll: u16,
    pub mode: Mode,
    pub status: String,
    quit: bool,
}

pub fn run(mut app: App) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = (|| {
        while !app.quit {
            terminal.draw(|frame| crate::ui::draw(frame, &mut app))?;
            if let Event::Key(key) = event::read()? {
                // Windows reports both press and release; act on press only.
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

impl App {
    pub fn new(store: Box<dyn VarStore>, write_enabled: bool) -> io::Result<Self> {
        let mut app = Self {
            store,
            write_enabled,
            vars: Vec::new(),
            filtered: Vec::new(),
            list_state: ListState::default(),
            filter: String::new(),
            view: View::Decoded,
            detail_scroll: 0,
            mode: Mode::Normal,
            status: String::new(),
            quit: false,
        };
        app.reload(None)?;
        app.status = format!(
            "{} variables from {}{}",
            app.vars.len(),
            app.store.describe(),
            if write_enabled { "  [WRITES ENABLED]" } else { "  [read-only]" }
        );
        Ok(app)
    }

    pub fn selected(&self) -> Option<&Variable> {
        self.list_state.selected().and_then(|i| self.filtered.get(i)).map(|&i| &self.vars[i])
    }

    /// Re-read every variable, keeping the selection on `keep` if given.
    fn reload(&mut self, keep: Option<&VarKey>) -> io::Result<()> {
        let keep = keep.cloned().or_else(|| self.selected().map(|v| v.key.clone()));
        let mut vars = self.store.list()?;
        // Group by vendor (EFI Global first), then by name.
        vars.sort_by(|a, b| {
            let rank = |v: &Variable| (v.key.vendor != guids::EFI_GLOBAL_VARIABLE, v.key.vendor.to_string());
            rank(a).cmp(&rank(b)).then_with(|| a.key.name.cmp(&b.key.name))
        });
        self.vars = vars;
        self.apply_filter(keep.as_ref());
        Ok(())
    }

    fn apply_filter(&mut self, keep: Option<&VarKey>) {
        let needle = self.filter.to_lowercase();
        self.filtered = (0..self.vars.len())
            .filter(|&i| {
                let v = &self.vars[i];
                needle.is_empty()
                    || v.key.name.to_lowercase().contains(&needle)
                    || v.key.vendor.to_string().contains(&needle)
                    || guids::vendor_name(&v.key.vendor).is_some_and(|n| n.to_lowercase().contains(&needle))
            })
            .collect();
        let pos = keep.and_then(|k| self.filtered.iter().position(|&i| &self.vars[i].key == k));
        self.list_state.select(match (pos, self.filtered.is_empty()) {
            (Some(p), _) => Some(p),
            (None, true) => None,
            (None, false) => Some(0),
        });
        self.detail_scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.filtered.len() as isize - 1) as usize;
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.on_normal_key(key),
            Mode::Filter => self.on_filter_key(key),
            Mode::HexEdit(ed) => self.on_hex_key(ed, key),
            Mode::OrderEdit(ed) => self.on_order_key(ed, key),
            Mode::Confirm(pending) => self.on_confirm_key(pending, key),
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX / 2),
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(10),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(10),
            KeyCode::Tab => {
                self.view = if self.view == View::Decoded { View::Hex } else { View::Decoded };
                self.detail_scroll = 0;
            }
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('r') => match self.reload(None) {
                Ok(()) => self.status = format!("reloaded {} variables", self.vars.len()),
                Err(e) => self.status = format!("reload failed: {e}"),
            },
            KeyCode::Char('D') => self.dump_all(),
            KeyCode::Char('e') => self.begin_edit(false),
            KeyCode::Char('x') => self.begin_edit(true),
            KeyCode::Char('a') => self.toggle_active(),
            _ => {}
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => return,
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => {}
        }
        if key.code != KeyCode::Esc {
            self.mode = Mode::Filter;
        }
        let keep = self.selected().map(|v| v.key.clone());
        self.apply_filter(keep.as_ref());
    }

    /// Why the selected variable cannot be written, if it can't.
    fn write_blocker(&self, var: &Variable) -> Option<String> {
        if !self.write_enabled {
            return Some("read-only mode: restart with --write to enable edits".into());
        }
        if attrs::is_authenticated(var.attributes) {
            return Some(format!(
                "{} is an authenticated variable ({}); writes need a signed payload and are not supported",
                var.key.name,
                attrs::describe(var.attributes)
            ));
        }
        None
    }

    fn begin_edit(&mut self, force_hex: bool) {
        let Some(var) = self.selected() else { return };
        if let Some(why) = self.write_blocker(var) {
            self.status = why;
            return;
        }
        let kind = classify(&var.key.vendor, &var.key.name);
        if !force_hex && kind == Some(Kind::OptionOrder)
            && let Ok(order) = load_option::parse_order(&var.data) {
                self.mode = Mode::OrderEdit(OrderEditor { order, selected: 0 });
                self.status = "↑↓ select · Shift+↑↓ / K J move · Del remove · Enter save · Esc cancel".into();
                return;
            }
        if var.data.is_empty() {
            self.status = "variable is empty; nothing to hex-edit".into();
            return;
        }
        self.mode = Mode::HexEdit(HexEditor { data: var.data.clone(), cursor: 0, high_nibble: true });
        self.view = View::Hex;
        self.status = "arrows move · 0-9 a-f overwrite · Enter save · Esc cancel".into();
    }

    fn toggle_active(&mut self) {
        let Some(var) = self.selected() else { return };
        if classify(&var.key.vendor, &var.key.name) != Some(Kind::LoadOption) {
            self.status = "'a' toggles LOAD_OPTION_ACTIVE on Boot####/Driver#### variables".into();
            return;
        }
        if let Some(why) = self.write_blocker(var) {
            self.status = why;
            return;
        }
        let mut opt = match LoadOption::parse(&var.data) {
            Ok(o) => o,
            Err(e) => {
                self.status = format!("cannot parse load option: {e}");
                return;
            }
        };
        opt.attributes ^= LOAD_OPTION_ACTIVE;
        let data = match opt.to_bytes() {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("cannot encode load option: {e}");
                return;
            }
        };
        let summary = vec![
            format!("\"{}\"", opt.description),
            format!("ACTIVE: {} → {}", !opt.is_active(), opt.is_active()),
        ];
        self.confirm(data, summary);
    }

    fn on_hex_key(&mut self, mut ed: HexEditor, key: KeyEvent) {
        let len = ed.data.len();
        match key.code {
            KeyCode::Esc => {
                self.status = "edit cancelled".into();
                return;
            }
            KeyCode::Enter => {
                let original = self.selected().map(|v| v.data.clone()).unwrap_or_default();
                let changed = original.iter().zip(&ed.data).filter(|(a, b)| a != b).count();
                if changed == 0 {
                    self.status = "no changes".into();
                    return;
                }
                self.confirm(ed.data, vec![format!("{changed} byte(s) changed")]);
                return;
            }
            KeyCode::Left => ed.cursor = ed.cursor.saturating_sub(1),
            KeyCode::Right => ed.cursor = (ed.cursor + 1).min(len - 1),
            KeyCode::Up => ed.cursor = ed.cursor.saturating_sub(16),
            KeyCode::Down => ed.cursor = (ed.cursor + 16).min(len - 1),
            KeyCode::Home => ed.cursor = 0,
            KeyCode::End => ed.cursor = len - 1,
            KeyCode::Char(c) if c.is_ascii_hexdigit() => {
                let nibble = c.to_digit(16).unwrap() as u8;
                let byte = &mut ed.data[ed.cursor];
                if ed.high_nibble {
                    *byte = (*byte & 0x0f) | (nibble << 4);
                } else {
                    *byte = (*byte & 0xf0) | nibble;
                    ed.cursor = (ed.cursor + 1).min(len - 1);
                }
                ed.high_nibble = !ed.high_nibble;
                self.mode = Mode::HexEdit(ed);
                return;
            }
            _ => {}
        }
        ed.high_nibble = true;
        self.detail_scroll = (ed.cursor / 16).saturating_sub(10) as u16;
        self.mode = Mode::HexEdit(ed);
    }

    fn on_order_key(&mut self, mut ed: OrderEditor, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let last = ed.order.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => {
                self.status = "edit cancelled".into();
                return;
            }
            KeyCode::Enter => {
                let data = load_option::encode_order(&ed.order);
                if Some(&data) == self.selected().map(|v| &v.data) {
                    self.status = "no changes".into();
                    return;
                }
                let names: Vec<String> = ed.order.iter().map(|n| format!("{n:04X}")).collect();
                self.confirm(data, vec![format!("new order: {}", names.join(", "))]);
                return;
            }
            KeyCode::Up if shift => move_item(&mut ed, -1),
            KeyCode::Down if shift => move_item(&mut ed, 1),
            KeyCode::Char('K') => move_item(&mut ed, -1),
            KeyCode::Char('J') => move_item(&mut ed, 1),
            KeyCode::Up | KeyCode::Char('k') => ed.selected = ed.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => ed.selected = (ed.selected + 1).min(last),
            KeyCode::Delete | KeyCode::Char('-') if !ed.order.is_empty() => {
                ed.order.remove(ed.selected);
                ed.selected = ed.selected.min(ed.order.len().saturating_sub(1));
            }
            _ => {}
        }
        self.mode = Mode::OrderEdit(ed);
    }

    fn confirm(&mut self, data: Vec<u8>, mut summary: Vec<String>) {
        let Some(var) = self.selected() else { return };
        // APPEND_WRITE is a write-time flag, never part of a stored variable.
        let new = Variable { key: var.key.clone(), attributes: var.attributes & !attrs::APPEND_WRITE, data };
        summary.insert(0, format!("Write {} ({} → {} bytes)?", var.key, var.data.len(), new.data.len()));
        summary.push(format!("Attributes: {}", attrs::describe(new.attributes)));
        summary.push("The current value is backed up first.".into());
        self.mode = Mode::Confirm(PendingWrite { var: new, summary });
    }

    fn on_confirm_key(&mut self, pending: PendingWrite, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.commit(pending.var),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.status = "write cancelled".into(),
            _ => self.mode = Mode::Confirm(pending),
        }
    }

    fn commit(&mut self, var: Variable) {
        let backup = match self.backup(&var.key) {
            Ok(path) => path,
            Err(e) => {
                self.status = format!("backup failed, nothing written: {e}");
                return;
            }
        };
        if let Err(e) = self.store.write(&var) {
            self.status = format!("write failed: {e} (backup in {})", backup.display());
            return;
        }
        let verified = self.store.read(&var.key).map(|v| v.data == var.data);
        if let Err(e) = self.reload(Some(&var.key)) {
            self.status = format!("written, but reload failed: {e}");
            return;
        }
        self.status = match verified {
            Ok(true) => format!("wrote {} and read it back OK; backup in {}", var.key.name, backup.display()),
            Ok(false) => format!("wrote {}, but read-back differs! backup in {}", var.key.name, backup.display()),
            Err(e) => format!("wrote {}, read-back failed: {e}; backup in {}", var.key.name, backup.display()),
        };
    }

    /// Save the current stored value to `efivars-backups/<millis>/`, in a
    /// layout that `--dir` can open directly.
    fn backup(&mut self, key: &VarKey) -> io::Result<PathBuf> {
        let current = self.store.read(key)?;
        let dir = PathBuf::from("efivars-backups").join(timestamp());
        DirStore::dump(&dir, &[current])?;
        Ok(dir)
    }

    fn dump_all(&mut self) {
        let dir = PathBuf::from(format!("efivars-dump-{}", timestamp()));
        self.status = match DirStore::dump(&dir, &self.vars) {
            Ok(n) => format!("dumped {n} variables to {}", dir.display()),
            Err(e) => format!("dump failed: {e}"),
        };
    }
}

fn move_item(ed: &mut OrderEditor, delta: isize) {
    let to = ed.selected as isize + delta;
    if to >= 0 && (to as usize) < ed.order.len() {
        ed.order.swap(ed.selected, to as usize);
        ed.selected = to as usize;
    }
}

fn timestamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use efivars_core::guids::EFI_GLOBAL_VARIABLE;
    use efivars_core::store::MemStore;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_key(KeyEvent::new(code, modifiers));
    }

    fn sample_store() -> MemStore {
        let var = |name: &str, attributes, data: &[u8]| Variable {
            key: VarKey::new(name, EFI_GLOBAL_VARIABLE),
            attributes,
            data: data.to_vec(),
        };
        MemStore {
            vars: vec![
                var("BootOrder", 7, &[1, 0, 0, 0]),
                var("Timeout", 7, &[5, 0]),
                var("PK", 0x27, &[0; 4]),
            ],
        }
    }

    fn render(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    }

    #[test]
    fn read_only_blocks_edits() {
        let mut app = App::new(Box::new(sample_store()), false).unwrap();
        render(&mut app);
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.contains("read-only"));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        render(&mut app);
    }

    #[test]
    fn authenticated_variables_are_refused() {
        let mut app = App::new(Box::new(sample_store()), true).unwrap();
        for c in "PK".chars() {
            press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        }
        assert_eq!(app.selected().unwrap().key.name, "PK");
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.status.contains("authenticated"), "{}", app.status);
    }

    #[test]
    fn reorder_boot_order_writes_and_backs_up() {
        let mut app = App::new(Box::new(sample_store()), true).unwrap();
        assert_eq!(app.selected().unwrap().key.name, "BootOrder");
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::OrderEdit(_)));
        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
        render(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::Confirm(_)));
        render(&mut app);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.status.contains("read it back OK"), "{}", app.status);
        assert_eq!(app.selected().unwrap().data, vec![0, 0, 1, 0]);
        let _ = std::fs::remove_dir_all("efivars-backups");
    }

    #[test]
    fn hex_edit_changes_bytes() {
        let mut app = App::new(Box::new(sample_store()), true).unwrap();
        press(&mut app, KeyCode::End, KeyModifiers::NONE); // sorted: BootOrder, PK, Timeout
        assert_eq!(app.selected().unwrap().key.name, "Timeout");
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('0'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        render(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.status, "write cancelled");
        assert_eq!(app.selected().unwrap().data, vec![5, 0]);
    }
}
