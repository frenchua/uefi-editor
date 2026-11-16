//! Single-line text input shared by the editors.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

pub struct LineInput {
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
}

pub enum InputEvent {
    Pending,
    Submit,
    Cancel,
}

impl LineInput {
    pub fn new(text: String) -> Self {
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(b, _)| b)
    }

    pub fn handle(&mut self, key: KeyEvent) -> InputEvent {
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Esc => return InputEvent::Cancel,
            KeyCode::Enter => return InputEvent::Submit,
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            _ => {}
        }
        InputEvent::Pending
    }

    /// The text as spans with the cursor cell highlighted.
    pub fn spans(&self) -> Vec<Span<'static>> {
        let chars: Vec<char> = self.text.chars().collect();
        let before: String = chars[..self.cursor].iter().collect();
        let at = chars.get(self.cursor).map_or(" ".to_string(), |c| c.to_string());
        let after: String = chars.get(self.cursor + 1..).map_or(String::new(), |s| s.iter().collect());
        vec![Span::raw(before), Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)), Span::raw(after)]
    }
}
