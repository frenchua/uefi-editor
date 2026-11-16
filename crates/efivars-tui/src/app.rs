//! Application state and key handling.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use efivars_core::decode::{self, Field, Kind, classify};
use efivars_core::devpath::DevicePath;
use efivars_core::load_option::{LOAD_OPTION_ACTIVE, LoadOption};
use efivars_core::store::DirStore;
use efivars_core::{VarKey, VarStore, Variable, attrs, form, guids, spec};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::form_editor::{self, FormEditor};
use crate::path_editor::{self, PathEditor};

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

pub struct PendingWrite {
    pub var: Variable,
    pub summary: Vec<String>,
}

pub enum Mode {
    Normal,
    Filter,
    HexEdit(HexEditor),
    PathEdit(PathEditor),
    FormEdit(FormEditor),
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
            Mode::PathEdit(ed) => self.on_path_key(ed, key),
            Mode::FormEdit(ed) => self.on_form_key(ed, key),
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
        if let Some(why) = spec::access(&var.key.vendor, &var.key.name).blocker(&var.key.name) {
            return Some(why);
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
        let mut fallback_note = None;
        if !force_hex && let Some(kind) = kind {
            // Only open structured editors on data that re-encodes exactly,
            // so an edit can never silently drop bytes.
            let structured = if kind == Kind::DevicePath {
                DevicePath::parse_exact(&var.data).map(|p| Mode::PathEdit(PathEditor::new(&p)))
            } else if form::has_form(kind) {
                form::build(kind, &var.key.name, &var.data, &self.vars).and_then(|fields| {
                    if form::encode(kind, &fields, &self.vars).as_ref() == Ok(&var.data) {
                        Ok(Mode::FormEdit(FormEditor::new(kind, fields)))
                    } else {
                        Err("its bytes are not in canonical form".into())
                    }
                })
            } else {
                Err(format!("{} has no editor", kind.name()))
            };
            match structured {
                Ok(mode) => {
                    self.status = format!("editing {} (x for raw hex)", var.key.name);
                    self.mode = mode;
                    self.view = View::Decoded;
                    self.detail_scroll = 0;
                    return;
                }
                Err(e) => fallback_note = Some(format!("cannot edit structurally ({e}); using hex editor")),
            }
        }
        if var.data.is_empty() {
            self.status = "variable is empty; nothing to hex-edit".into();
            return;
        }
        self.mode = Mode::HexEdit(HexEditor { data: var.data.clone(), cursor: 0, high_nibble: true });
        self.view = View::Hex;
        self.status = fallback_note.unwrap_or_else(|| "arrows move · 0-9 a-f overwrite · Enter save · Esc cancel".into());
    }

    fn on_path_key(&mut self, mut ed: PathEditor, key: KeyEvent) {
        match ed.on_key(key) {
            path_editor::Outcome::Continue => self.mode = Mode::PathEdit(ed),
            path_editor::Outcome::Cancel => self.status = "edit cancelled".into(),
            path_editor::Outcome::Save(path) => match path.to_bytes() {
                Ok(data) if Some(&data) == self.selected().map(|v| &v.data) => {
                    ed.error = Some("no changes to save".into());
                    self.mode = Mode::PathEdit(ed);
                }
                Ok(data) => {
                    let summary = vec![format!("old: {}", ed.original_text), format!("new: {}", path.to_text())];
                    self.confirm(data, summary);
                }
                Err(e) => {
                    ed.error = Some(e);
                    self.mode = Mode::PathEdit(ed);
                }
            },
        }
    }

    fn on_form_key(&mut self, mut ed: FormEditor, key: KeyEvent) {
        match ed.on_key(key, &self.vars) {
            form_editor::Outcome::Continue => self.mode = Mode::FormEdit(ed),
            form_editor::Outcome::Cancel => self.status = "edit cancelled".into(),
            form_editor::Outcome::Save(data) => {
                let Some(var) = self.selected() else { return };
                if data == var.data {
                    ed.error = Some("no changes to save".into());
                    self.mode = Mode::FormEdit(ed);
                } else {
                    let summary = change_summary(var, &data);
                    self.confirm(data, summary);
                }
            }
        }
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

/// Lines of the decoded view that a change adds (+) or removes (-), so the
/// confirmation dialog shows what the edit means rather than raw bytes.
fn change_summary(old: &Variable, new_data: &[u8]) -> Vec<String> {
    let lines = |data: &[u8]| -> Vec<String> {
        let v = Variable { key: old.key.clone(), attributes: old.attributes, data: data.to_vec() };
        match decode::decode(&v) {
            Some(Ok(d)) => {
                let mut flat = Vec::new();
                Field::flatten(&d.fields, 0, &mut flat);
                flat.into_iter().map(|(depth, f)| format!("{}{}: {}", "  ".repeat(depth), f.label, f.value)).collect()
            }
            _ => Vec::new(),
        }
    };
    let (before, after) = (lines(&old.data), lines(new_data));
    let mut out: Vec<String> = before.iter().filter(|l| !after.contains(l)).map(|l| format!("- {}", l.trim_start())).collect();
    out.extend(after.iter().filter(|l| !before.contains(l)).map(|l| format!("+ {}", l.trim_start())));
    if out.len() > 12 {
        let more = out.len() - 12;
        out.truncate(12);
        out.push(format!("… and {more} more changed line(s)"));
    }
    out
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

    /// Draw one frame and return the screen as text, one line per row.
    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect()
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn clear_input(app: &mut App) {
        for _ in 0..200 {
            press(app, KeyCode::Backspace, KeyModifiers::NONE);
        }
    }

    fn path(texts: &[&[&str]]) -> DevicePath {
        DevicePath {
            instances: texts
                .iter()
                .map(|inst| inst.iter().map(|t| efivars_core::devpath::Node::from_text(t).unwrap()).collect())
                .collect(),
        }
    }

    fn path_store() -> MemStore {
        let con_in = path(&[
            &["PciRoot(0x0)", "Pci(0x14,0x0)", "USB(0x3,0x0)"],
            &["PciRoot(0x0)", "Pci(0x14,0x0)", "USB(0xd,0x0)"],
        ]);
        let boot = LoadOption {
            attributes: LOAD_OPTION_ACTIVE,
            description: "gentoo".into(),
            file_path_list: path(&[&[
                "HD(1,GPT,01234567-89ab-cdef-0123-456789abcdef,0x800,0x64000)",
                r"\EFI\gentoo\grubx64.efi",
            ]])
            .to_bytes()
            .unwrap(),
            optional_data: vec![1, 2, 3],
        };
        let var = |name: &str, data: Vec<u8>| Variable { key: VarKey::new(name, EFI_GLOBAL_VARIABLE), attributes: 7, data };
        MemStore { vars: vec![var("Boot0001", boot.to_bytes().unwrap()), var("ConIn", con_in.to_bytes().unwrap())] }
    }

    fn select(app: &mut App, name: &str) {
        let pos = app.filtered.iter().position(|&i| app.vars[i].key.name == name).unwrap();
        app.list_state.select(Some(pos));
    }

    #[test]
    fn edits_con_in_as_device_path_text() {
        let mut app = App::new(Box::new(path_store()), true).unwrap();
        select(&mut app, "ConIn");
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::PathEdit(_)), "{}", app.status);
        let screen = render(&mut app);
        assert!(screen.contains("USB(0xd,0x0)") && screen.contains("instance 1"), "{screen}");

        // Rows: 0 PciRoot, 1 Pci, 2 USB(3), 3 separator, 4 PciRoot, 5 Pci, 6 USB(d).
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(render(&mut app).contains("› edit USB(0x3,0x0)"));
        clear_input(&mut app);
        type_text(&mut app, "USB(5)"); // wrong arity: rejected, input stays open
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(render(&mut app).contains("USB takes 2 argument(s), got 1"));
        press(&mut app, KeyCode::Left, KeyModifiers::NONE); // before ')'
        type_text(&mut app, ",0");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        render(&mut app);

        // Add a third instance at the end: separator, then a node.
        press(&mut app, KeyCode::End, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE); // empty instance: refused
        assert!(render(&mut app).contains("instance 2 is empty"));
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        type_text(&mut app, "PcieRoot(0x1)");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        render(&mut app);

        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        let Mode::Confirm(pending) = &app.mode else { panic!("expected confirm, status: {}", app.status) };
        assert!(
            pending.summary.iter().any(|l| l.starts_with("new: PciRoot(0x0)/Pci(0x14,0x0)/USB(0x5,0x0),")),
            "{:?}",
            pending.summary
        );
        render(&mut app);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.status.contains("read it back OK"), "{}", app.status);

        let expected = path(&[
            &["PciRoot(0x0)", "Pci(0x14,0x0)", "USB(0x5,0x0)"],
            &["PciRoot(0x0)", "Pci(0x14,0x0)", "USB(0xd,0x0)"],
            &["PcieRoot(0x1)"],
        ]);
        assert_eq!(app.selected().unwrap().data, expected.to_bytes().unwrap());
        let _ = std::fs::remove_dir_all("efivars-backups");
    }

    fn key(app: &mut App, code: KeyCode) {
        press(app, code, KeyModifiers::NONE);
    }

    #[test]
    fn edits_boot_entry_as_a_form() {
        let mut app = App::new(Box::new(path_store()), true).unwrap();
        select(&mut app, "Boot0001");
        key(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.mode, Mode::FormEdit(_)), "{}", app.status);
        let screen = render(&mut app);
        assert!(screen.contains("[x] ACTIVE") && screen.contains("\"gentoo\"") && screen.contains("+ add"), "{screen}");

        // Rows: 0 Attributes, 1-3 flags, 4 Category, 5 Description,
        // 6 File paths, 7 path [0], 8 add path, 9 Optional data.
        // Starts on row 1 (ACTIVE): turn it off.
        key(&mut app, KeyCode::Char(' '));
        // Description.
        for _ in 0..4 {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Enter);
        clear_input(&mut app);
        type_text(&mut app, "Gentoo Linux");
        key(&mut app, KeyCode::Enter);
        // File path [0]: opens the embedded device-path editor.
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(render(&mut app).contains("Editing File paths [0]"));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        clear_input(&mut app);
        type_text(&mut app, r"\EFI\gentoo\shimx64.efi");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('s')); // back to the form
        let Mode::FormEdit(ed) = &app.mode else { panic!("expected form: {}", app.status) };
        assert!(ed.sub.is_none());
        let efivars_core::form::Value::List { items, .. } = &ed.fields[3].value else { panic!() };
        assert!(items[0].display().ends_with(r"\EFI\gentoo\shimx64.efi"), "{}", items[0].display());

        key(&mut app, KeyCode::Char('s'));
        let Mode::Confirm(pending) = &app.mode else { panic!("expected confirm: {}", app.status) };
        let summary = pending.summary.join("\n");
        assert!(summary.contains("+ Description: Gentoo Linux"), "{summary}");
        assert!(summary.contains("- Description: gentoo"), "{summary}");
        render(&mut app);
        key(&mut app, KeyCode::Char('y'));
        assert!(app.status.contains("read it back OK"), "{}", app.status);

        let opt = LoadOption::parse(&app.selected().unwrap().data).unwrap();
        assert_eq!(opt.description.to_string(), "Gentoo Linux");
        assert_eq!(opt.optional_data, [1, 2, 3], "untouched fields are kept");
        assert!(!opt.is_active());
        assert!(opt.device_path_text().ends_with(r"\EFI\gentoo\shimx64.efi"), "{}", opt.device_path_text());
        let _ = std::fs::remove_dir_all("efivars-backups");
    }

    fn global_var(name: &str, data: &[u8]) -> Variable {
        Variable { key: VarKey::new(name, EFI_GLOBAL_VARIABLE), attributes: 7, data: data.to_vec() }
    }

    #[test]
    fn edits_timeout_as_a_number() {
        let store = MemStore { vars: vec![global_var("Timeout", &[5, 0])] };
        let mut app = App::new(Box::new(store), true).unwrap();
        key(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.mode, Mode::FormEdit(_)), "{}", app.status);
        key(&mut app, KeyCode::Enter);
        clear_input(&mut app);
        type_text(&mut app, "70000");
        key(&mut app, KeyCode::Enter);
        assert!(render(&mut app).contains("does not fit in 16 bits"));
        clear_input(&mut app);
        type_text(&mut app, "10");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.selected().unwrap().data, [10, 0]);
        let _ = std::fs::remove_dir_all("efivars-backups");
    }

    #[test]
    fn edits_os_indications_flags() {
        let store = MemStore {
            vars: vec![
                global_var("OsIndications", &0u64.to_le_bytes()),
                global_var("OsIndicationsSupported", &1u64.to_le_bytes()),
            ],
        };
        let mut app = App::new(Box::new(store), true).unwrap();
        select(&mut app, "OsIndications");
        key(&mut app, KeyCode::Char('e'));
        assert!(render(&mut app).contains("[ ] TIMESTAMP_REVOCATION  (not supported by this firmware)"));
        key(&mut app, KeyCode::Char(' ')); // BOOT_TO_FW_UI
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.selected().unwrap().data, 1u64.to_le_bytes());
        let _ = std::fs::remove_dir_all("efivars-backups");
    }

    #[test]
    fn spec_read_only_variables_are_refused() {
        let store = MemStore { vars: vec![global_var("BootCurrent", &[1, 0]), global_var("SecureBoot", &[1])] };
        let mut app = App::new(Box::new(store), true).unwrap();
        for k in ['e', 'x'] {
            key(&mut app, KeyCode::Char(k));
            assert!(matches!(app.mode, Mode::Normal));
            assert!(app.status.contains("read-only per the UEFI spec"), "{}", app.status);
        }
    }

    #[test]
    fn missing_option_reference_is_flagged() {
        let store = MemStore { vars: vec![global_var("BootNext", &[9, 0])] };
        let mut app = App::new(Box::new(store), true).unwrap();
        key(&mut app, KeyCode::Char('e'));
        assert!(render(&mut app).contains("Boot0009  (no such option)"));
    }

    #[test]
    fn unchanged_or_cancelled_path_edit_writes_nothing() {
        let mut app = App::new(Box::new(path_store()), true).unwrap();
        select(&mut app, "ConIn");
        let before = app.selected().unwrap().data.clone();
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(render(&mut app).contains("no changes to save"));
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.selected().unwrap().data, before);
    }

    /// Opens every spec-writable variable of a real dump (`EFIVARS_DUMP=<dir>`)
    /// in its structured editor, printing a couple of screens. Skips otherwise.
    #[test]
    fn real_dump_opens_in_structured_editors() {
        let Some(dir) = std::env::var_os("EFIVARS_DUMP") else { return };
        let mut app = App::new(Box::new(DirStore::new(dir)), true).unwrap();
        let names: Vec<String> = app
            .vars
            .iter()
            .filter(|v| {
                classify(&v.key.vendor, &v.key.name).is_some_and(|k| k == Kind::DevicePath || form::has_form(k))
                    && spec::access(&v.key.vendor, &v.key.name) == spec::Access::Editable
            })
            .map(|v| v.key.name.clone())
            .collect();
        for name in &names {
            select(&mut app, name);
            press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
            assert!(matches!(app.mode, Mode::PathEdit(_) | Mode::FormEdit(_)), "{name}: {}", app.status);
            let screen = render(&mut app);
            if name == "Boot0000" || name == "PlatformLang" {
                println!("{screen}");
            }
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        }
        println!("opened {} variables in structured editors: {}", names.len(), names.join(", "));
    }

    #[test]
    fn non_canonical_path_falls_back_to_hex() {
        let mut store = path_store();
        store.vars[1].data.extend([0xde, 0xad]); // trailing bytes after End-Entire
        let mut app = App::new(Box::new(store), true).unwrap();
        select(&mut app, "ConIn");
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::HexEdit(_)));
        assert!(app.status.contains("trailing"), "{}", app.status);
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
        assert!(app.status.contains("Secure Boot variable") && app.status.contains("TODO.md"), "{}", app.status);
    }

    #[test]
    fn reorder_boot_order_writes_and_backs_up() {
        let mut app = App::new(Box::new(sample_store()), true).unwrap();
        assert_eq!(app.selected().unwrap().key.name, "BootOrder");
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::FormEdit(_)), "{}", app.status);
        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT); // first entry moves down
        render(&mut app);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(matches!(app.mode, Mode::Confirm(_)), "{}", app.status);
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
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE); // raw hex, bypassing the form
        press(&mut app, KeyCode::Char('0'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        render(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.status, "write cancelled");
        assert_eq!(app.selected().unwrap().data, vec![5, 0]);
    }
}
