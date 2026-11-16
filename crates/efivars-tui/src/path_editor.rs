//! Structured editor for device paths (UEFI spec §10): used directly for
//! ConIn/ConOut/ErrOut, and embedded in the form editor for the file paths of
//! Boot####/Driver####/SysPrep#### load options.
//!
//! The path is shown as a flat list of nodes with instance separators between
//! them (the End-Instance nodes of the binary form). Each node is edited as
//! UEFI device-path text (§10.6), e.g. `Pci(0x14,0x0)` or `USB(0x3,0x0)`, and
//! parsed back with [`Node::from_text`].

use efivars_core::devpath::{DevicePath, Node};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::line_input::{InputEvent, LineInput};

pub enum Entry {
    Node(Node),
    /// Boundary between two device-path instances.
    Separator,
}

/// Where a line being typed will go when committed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Replace(usize),
    Insert(usize),
}

pub struct PathEditor {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub input: Option<(LineInput, Target)>,
    pub error: Option<String>,
    /// Text of the path before editing, for the confirmation summary.
    pub original_text: String,
}

pub enum Outcome {
    Continue,
    Cancel,
    Save(DevicePath),
}

/// Quick reference shown under the editor.
pub const SYNTAX_HELP: &[&str] = &[
    "PciRoot(uid) PcieRoot(uid) Acpi(PNP0A03,uid) AcpiAdr(adr) Pci(dev,fn) Ctrl(n)",
    "USB(port,intf) Sata(hba,pm,lun) NVMe(nsid,eui) Scsi(pun,lun) MAC(hex,type) Uri(url)",
    "HD(part,GPT|MBR,sig,start,size) CDROM(entry,start,size) \\EFI\\path\\file.efi",
    "VenHw|VenMsg|VenMedia(guid[,hex]) Path(type,subtype,hex) for anything else",
];

impl PathEditor {
    pub fn new(path: &DevicePath) -> Self {
        let mut entries = Vec::new();
        for (i, nodes) in path.instances.iter().enumerate() {
            if i > 0 {
                entries.push(Entry::Separator);
            }
            entries.extend(nodes.iter().cloned().map(Entry::Node));
        }
        Self { entries, selected: 0, input: None, error: None, original_text: path.to_text() }
    }

    /// Rebuild the device path, validating its instance structure.
    pub fn to_path(&self) -> Result<DevicePath, String> {
        let mut instances = vec![Vec::new()];
        for entry in &self.entries {
            match entry {
                Entry::Node(n) => instances.last_mut().unwrap().push(n.clone()),
                Entry::Separator => instances.push(Vec::new()),
            }
        }
        if instances.iter().all(Vec::is_empty) {
            return Err("device path has no nodes".into());
        }
        if let Some(i) = instances.iter().position(Vec::is_empty) {
            return Err(format!("instance {i} is empty; delete the extra separator"));
        }
        Ok(DevicePath { instances })
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        if let Some((input, target)) = &mut self.input {
            match input.handle(key) {
                InputEvent::Pending => {}
                InputEvent::Cancel => {
                    self.input = None;
                    self.error = None;
                }
                InputEvent::Submit => match Node::from_text(&input.text) {
                    Ok(node) => {
                        match *target {
                            Target::Replace(i) => self.entries[i] = Entry::Node(node),
                            Target::Insert(i) => {
                                self.entries.insert(i, Entry::Node(node));
                                self.selected = i;
                            }
                        }
                        self.input = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                },
            }
            return Outcome::Continue;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let last = self.entries.len().saturating_sub(1);
        self.error = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancel,
            KeyCode::Char('s') => match self.to_path() {
                Ok(path) => return Outcome::Save(path),
                Err(e) => self.error = Some(e),
            },
            KeyCode::Up if shift => self.move_selected(-1),
            KeyCode::Down if shift => self.move_selected(1),
            KeyCode::Char('K') => self.move_selected(-1),
            KeyCode::Char('J') => self.move_selected(1),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Enter | KeyCode::Char('e') => match self.entries.get(self.selected) {
                Some(Entry::Node(n)) => self.start_input(n.edit_text(), Target::Replace(self.selected)),
                Some(Entry::Separator) => self.error = Some("separators have no text; press d to delete".into()),
                None => self.start_input(String::new(), Target::Insert(0)),
            },
            KeyCode::Char('a') | KeyCode::Insert => {
                let at = if self.entries.is_empty() { 0 } else { self.selected + 1 };
                self.start_input(String::new(), Target::Insert(at));
            }
            KeyCode::Char('i') => {
                let at = if self.entries.is_empty() { 0 } else { self.selected + 1 };
                self.entries.insert(at, Entry::Separator);
                self.selected = at;
            }
            KeyCode::Delete | KeyCode::Char('d') if !self.entries.is_empty() => {
                self.entries.remove(self.selected);
                self.selected = self.selected.min(self.entries.len().saturating_sub(1));
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn start_input(&mut self, text: String, target: Target) {
        self.input = Some((LineInput::new(text), target));
        self.error = None;
    }

    fn move_selected(&mut self, delta: isize) {
        let to = self.selected as isize + delta;
        if to >= 0 && (to as usize) < self.entries.len() {
            self.entries.swap(self.selected, to as usize);
            self.selected = to as usize;
        }
    }
}
