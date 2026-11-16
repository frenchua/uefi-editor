//! Generic editor for the typed forms built by [`efivars_core::form`].
//!
//! Each field is one row. Flag sets expand into one checkbox row per named
//! bit, and lists expand into one row per item plus an "add" row. Device-path
//! values open the embedded [`PathEditor`].

use efivars_core::Variable;
use efivars_core::decode::Kind;
use efivars_core::devpath::DevicePath;
use efivars_core::form::{self, Field, Value};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::line_input::{InputEvent, LineInput};
use crate::path_editor::{self, PathEditor};

/// One selectable row of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Field(usize),
    /// Named bit `j` of the flags field `i`.
    Flag(usize, usize),
    /// Item `j` of the list field `i`.
    Item(usize, usize),
    /// "Add item" row of list field `i`.
    Add(usize),
}

pub struct FormEditor {
    pub kind: Kind,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub input: Option<(LineInput, Row)>,
    /// Embedded device-path editor and the row it will write back to.
    pub sub: Option<(PathEditor, Row)>,
    pub error: Option<String>,
}

pub enum Outcome {
    Continue,
    Cancel,
    Save(Vec<u8>),
}

impl FormEditor {
    pub fn new(kind: Kind, fields: Vec<Field>) -> Self {
        let mut ed = Self { kind, fields, selected: 0, input: None, sub: None, error: None };
        // Start on the first row that can be edited.
        ed.selected = ed.rows().iter().position(|r| !matches!(r, Row::Field(i) if ed.is_container(*i))).unwrap_or(0);
        ed
    }

    fn is_container(&self, i: usize) -> bool {
        matches!(self.fields[i].value, Value::Flags { .. } | Value::List { .. })
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            rows.push(Row::Field(i));
            match &f.value {
                Value::Flags { defs, .. } if f.editable => rows.extend((0..defs.len()).map(|j| Row::Flag(i, j))),
                Value::List { items, max, .. } => {
                    rows.extend((0..items.len()).map(|j| Row::Item(i, j)));
                    if f.editable && max.is_none_or(|m| items.len() < m) {
                        rows.push(Row::Add(i));
                    }
                }
                _ => {}
            }
        }
        rows
    }

    pub fn selected_row(&self) -> Row {
        let rows = self.rows();
        rows[self.selected.min(rows.len() - 1)]
    }

    /// The field the selected row belongs to.
    pub fn selected_field(&self) -> &Field {
        let (Row::Field(i) | Row::Flag(i, _) | Row::Item(i, _) | Row::Add(i)) = self.selected_row();
        &self.fields[i]
    }

    fn value_mut(&mut self, row: Row) -> Option<&mut Value> {
        match row {
            Row::Field(i) => Some(&mut self.fields[i].value),
            Row::Item(i, j) => match &mut self.fields[i].value {
                Value::List { items, .. } => items.get_mut(j),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, vars: &[Variable]) -> Outcome {
        if self.sub.is_some() {
            self.on_sub_key(key);
            return Outcome::Continue;
        }
        if self.input.is_some() {
            self.on_input_key(key);
            return Outcome::Continue;
        }
        self.error = None;
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let last = self.rows().len() - 1;
        let row = self.selected_row();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancel,
            KeyCode::Char('s') => match form::encode(self.kind, &self.fields, vars) {
                Ok(data) => return Outcome::Save(data),
                Err(e) => self.error = Some(e),
            },
            KeyCode::Up if shift => self.move_item(-1),
            KeyCode::Down if shift => self.move_item(1),
            KeyCode::Char('K') => self.move_item(-1),
            KeyCode::Char('J') => self.move_item(1),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Left | KeyCode::Right => {
                let forward = key.code == KeyCode::Right;
                if !self.value_mut(row).is_some_and(|v| v.cycle(forward)) {
                    self.error = Some("←/→ change choice fields".into());
                }
            }
            KeyCode::Char('t') => {
                if let Some(v) = self.value_mut(row)
                    && let Err(e) = v.toggle_data_view()
                {
                    self.error = Some(e);
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => self.delete_item(row),
            KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char(' ') => self.activate(row),
            _ => {}
        }
        Outcome::Continue
    }

    /// Enter/Space on a row: toggle, cycle, or open the right editor.
    fn activate(&mut self, row: Row) {
        let (Row::Field(i) | Row::Flag(i, _) | Row::Item(i, _) | Row::Add(i)) = row;
        if !self.fields[i].editable {
            self.error = Some(format!("{} is computed automatically when saving", self.fields[i].label));
            return;
        }
        match row {
            Row::Flag(i, j) => {
                if let Value::Flags { value, defs } = &mut self.fields[i].value {
                    *value ^= defs[j].mask;
                }
            }
            Row::Add(i) => {
                let Value::List { template, .. } = &self.fields[i].value else { return };
                match template.as_ref() {
                    Value::DevicePath(p) => self.sub = Some((PathEditor::new(p), row)),
                    _ => self.input = Some((LineInput::new(String::new()), row)),
                }
            }
            Row::Field(_) | Row::Item(..) => {
                let Some(value) = self.value_mut(row) else { return };
                match value {
                    Value::Bool(_) => {
                        value.toggle();
                    }
                    Value::Choice { .. } => {
                        value.cycle(true);
                    }
                    Value::DevicePath(p) => {
                        let ed = PathEditor::new(p);
                        self.sub = Some((ed, row));
                    }
                    Value::Flags { .. } => self.error = Some("select one of the flags below and press Space".into()),
                    Value::List { .. } => self.error = Some("select an item below, or the + row to add one".into()),
                    v if v.is_text_editable() => {
                        let text = v.edit_text();
                        self.input = Some((LineInput::new(text), row));
                    }
                    _ => {}
                }
            }
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        let (input, row) = self.input.as_mut().unwrap();
        let row = *row;
        match input.handle(key) {
            InputEvent::Pending => {}
            InputEvent::Cancel => {
                self.input = None;
                self.error = None;
            }
            InputEvent::Submit => {
                let text = input.text.clone();
                let result = match row {
                    Row::Add(i) => {
                        let Value::List { items, template, .. } = &mut self.fields[i].value else { return };
                        let mut item = template.as_ref().clone();
                        item.set_text(&text).map(|()| items.push(item))
                    }
                    _ => match self.value_mut(row) {
                        Some(v) => v.set_text(&text),
                        None => Ok(()),
                    },
                };
                match result {
                    Ok(()) => {
                        self.input = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
    }

    fn on_sub_key(&mut self, key: KeyEvent) {
        let (ed, row) = self.sub.as_mut().unwrap();
        let row = *row;
        match ed.on_key(key) {
            path_editor::Outcome::Continue => {}
            path_editor::Outcome::Cancel => self.sub = None,
            path_editor::Outcome::Save(path) => {
                self.store_path(row, path);
                self.sub = None;
            }
        }
    }

    fn store_path(&mut self, row: Row, path: DevicePath) {
        match row {
            Row::Add(i) => {
                if let Value::List { items, .. } = &mut self.fields[i].value {
                    items.push(Value::DevicePath(path));
                }
            }
            _ => {
                if let Some(v) = self.value_mut(row) {
                    *v = Value::DevicePath(path);
                }
            }
        }
    }

    fn delete_item(&mut self, row: Row) {
        let Row::Item(i, j) = row else {
            self.error = Some("d deletes list items".into());
            return;
        };
        if let Value::List { items, .. } = &mut self.fields[i].value {
            items.remove(j);
        }
        self.selected = self.selected.min(self.rows().len() - 1);
    }

    fn move_item(&mut self, delta: isize) {
        let Row::Item(i, j) = self.selected_row() else { return };
        let Value::List { items, .. } = &mut self.fields[i].value else { return };
        let to = j as isize + delta;
        if to >= 0 && (to as usize) < items.len() {
            items.swap(j, to as usize);
            self.selected = (self.selected as isize + delta) as usize;
        }
    }
}
