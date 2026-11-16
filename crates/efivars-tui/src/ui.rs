//! Rendering.

use efivars_core::decode::{self, Field};
use efivars_core::{Variable, attrs, guids, hex};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};

use efivars_core::form::{self, Value};

use crate::app::{App, HexEditor, Mode, View};
use crate::form_editor::{FormEditor, Row};
use crate::line_input::LineInput;
use crate::path_editor::{Entry, PathEditor, SYNTAX_HELP, Target};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).areas(main);

    draw_list(frame, app, left);
    draw_detail(frame, app, right);
    draw_status(frame, app, status);
    if let Mode::Confirm(pending) = &app.mode {
        draw_confirm(frame, &pending.summary);
    }
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&i| {
            let v = &app.vars[i];
            let vendor = guids::vendor_name(&v.key.vendor)
                .map(str::to_string)
                .unwrap_or_else(|| v.key.vendor.to_string()[..8].to_string());
            ListItem::new(Line::from(vec![
                Span::raw(v.key.name.clone()),
                Span::raw("  "),
                Span::styled(vendor, Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let filter = if matches!(app.mode, Mode::Filter) || !app.filter.is_empty() {
        format!(" /{}{} ", app.filter, if matches!(app.mode, Mode::Filter) { "▏" } else { "" })
    } else {
        String::new()
    };
    let title = format!(" Variables {}/{} {filter}", app.filtered.len(), app.vars.len());
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(var) = app.selected() else {
        frame.render_widget(Paragraph::new("no variable selected").block(Block::bordered()), area);
        return;
    };

    let mut lines = header_lines(var);
    let view_name = match (&app.mode, app.view) {
        (Mode::HexEdit(ed), _) => {
            lines.extend(hex_edit_lines(&var.data, ed));
            "hex edit"
        }
        (Mode::FormEdit(ed), _) => {
            lines.extend(form_edit_lines(app, ed));
            "edit"
        }
        (Mode::PathEdit(ed), _) => {
            lines.extend(path_edit_lines(ed));
            "device path edit"
        }
        (_, View::Hex) => {
            lines.extend(hex::dump_lines(&var.data).into_iter().map(Line::from));
            "hex"
        }
        (_, View::Decoded) => {
            lines.extend(decoded_lines(var));
            "decoded"
        }
    };

    let title = format!(" {} — {view_name} (Tab toggles) ", var.key.name);
    let para = Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    frame.render_widget(para, area);
}

fn header_lines(var: &Variable) -> Vec<Line<'static>> {
    let label = |s: &str| Span::styled(format!("{s:<11}"), Style::new().fg(Color::Cyan));
    let vendor = match guids::vendor_name(&var.key.vendor) {
        Some(n) => format!("{} ({n})", var.key.vendor),
        None => var.key.vendor.to_string(),
    };
    let kind = decode::classify(&var.key.vendor, &var.key.name).map(|k| k.name()).unwrap_or("unknown (raw bytes)");
    let mut attr_line = vec![label("Attributes"), Span::raw(format!("0x{:x} {}", var.attributes, attrs::describe(var.attributes)))];
    if attrs::is_authenticated(var.attributes) {
        attr_line.push(Span::styled("  authenticated: read-only", Style::new().fg(Color::Yellow)));
    }
    vec![
        Line::from(vec![label("Vendor"), Span::raw(vendor)]),
        Line::from(attr_line),
        Line::from(vec![label("Size"), Span::raw(format!("{} bytes", var.data.len()))]),
        Line::from(vec![label("Type"), Span::raw(kind)]),
        Line::default(),
    ]
}

fn decoded_lines(var: &Variable) -> Vec<Line<'static>> {
    match decode::decode(var) {
        None => {
            let mut lines = vec![Line::from("No decoder for this variable; showing raw bytes.".italic()), Line::default()];
            lines.extend(hex::dump_lines(&var.data).into_iter().map(Line::from));
            lines
        }
        Some(Err(e)) => vec![Line::from(format!("Parse error: {e}").red()), Line::from("Press Tab for the hex view.")],
        Some(Ok(d)) => {
            let mut flat = Vec::new();
            Field::flatten(&d.fields, 0, &mut flat);
            flat.into_iter()
                .map(|(depth, f)| {
                    Line::from(vec![
                        Span::raw("  ".repeat(depth)),
                        Span::styled(f.label.clone(), Style::new().bold()),
                        Span::raw(if f.value.is_empty() { String::new() } else { format!(": {}", f.value) }),
                    ])
                })
                .collect()
        }
    }
}

fn hex_edit_lines(original: &[u8], ed: &HexEditor) -> Vec<Line<'static>> {
    ed.data
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let mut spans = vec![Span::raw(format!("{:04x}  ", row * 16))];
            for (col, &b) in chunk.iter().enumerate() {
                let i = row * 16 + col;
                let mut style = Style::new();
                if original.get(i) != Some(&b) {
                    style = style.fg(Color::Yellow).bold();
                }
                if i == ed.cursor {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                spans.push(Span::styled(format!("{b:02x}"), style));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::raw(" ".repeat((16 - chunk.len()) * 3 + 1)));
            spans.push(Span::raw(chunk.iter().map(|&b| hex::printable(b)).collect::<String>()));
            Line::from(spans)
        })
        .collect()
}

fn input_line(prefix: String, input: &LineInput) -> Line<'static> {
    let mut spans = vec![Span::styled(prefix, Style::new().fg(Color::Yellow).bold())];
    spans.extend(input.spans());
    Line::from(spans)
}

/// Description of the load option a reference points to, for display.
fn option_note(app: &App, value: &Value) -> String {
    match value {
        Value::OptionRef { number, prefix } => match form::option_description(&app.vars, prefix, *number) {
            Some(d) => format!("  \"{d}\""),
            None => "  (no such option)".into(),
        },
        _ => String::new(),
    }
}

fn form_edit_lines(app: &App, ed: &FormEditor) -> Vec<Line<'static>> {
    if let Some((sub, row)) = &ed.sub {
        let (Row::Field(i) | Row::Flag(i, _) | Row::Item(i, _) | Row::Add(i)) = *row;
        let what = match row {
            Row::Item(_, j) => format!("{} [{j}]", ed.fields[i].label),
            Row::Add(_) => format!("new entry in {}", ed.fields[i].label),
            _ => ed.fields[i].label.clone(),
        };
        let mut lines = vec![Line::styled(format!("Editing {what} (s returns to the form)"), Style::new().bold()), Line::default()];
        lines.extend(path_edit_lines(sub));
        return lines;
    }
    let dim = Style::new().fg(Color::DarkGray);
    let selected = ed.selected_row();
    let mut lines = Vec::new();
    for row in ed.rows() {
        if let Some((input, _)) = ed.input.as_ref().filter(|(_, r)| *r == row) {
            let label = match row {
                Row::Item(i, j) => format!("  {} [{j}] › ", ed.fields[i].label),
                Row::Add(i) => format!("  {} + › ", ed.fields[i].label),
                Row::Field(i) | Row::Flag(i, _) => format!("{} › ", ed.fields[i].label),
            };
            lines.push(input_line(label, input));
            continue;
        }
        let line = match row {
            Row::Field(i) => {
                let f = &ed.fields[i];
                let mut spans = vec![Span::styled(format!("{:<20}", f.label), Style::new().bold())];
                match &f.value {
                    Value::List { .. } => {}
                    v => {
                        spans.push(Span::raw(v.display()));
                        spans.push(Span::styled(option_note(app, v), dim));
                    }
                }
                if !f.editable {
                    spans.push(Span::styled("  (computed)", dim));
                }
                Line::from(spans)
            }
            Row::Flag(i, j) => {
                let Value::Flags { value, defs } = &ed.fields[i].value else { continue };
                let d = &defs[j];
                let mark = if value & d.mask != 0 { "[x]" } else { "[ ]" };
                let note = if d.note.is_empty() { String::new() } else { format!("  ({})", d.note) };
                Line::from(vec![Span::raw(format!("    {mark} {}", d.name)), Span::styled(note, dim)])
            }
            Row::Item(i, j) => {
                let Value::List { items, .. } = &ed.fields[i].value else { continue };
                Line::from(vec![
                    Span::raw(format!("    [{j}] {}", items[j].display())),
                    Span::styled(option_note(app, &items[j]), dim),
                ])
            }
            Row::Add(_) => Line::styled("    + add…", dim),
        };
        lines.push(if row == selected && ed.input.is_none() { line.reversed() } else { line });
    }
    lines.push(Line::default());
    if let Some(e) = &ed.error {
        lines.push(Line::styled(format!("✗ {e}"), Style::new().fg(Color::Red)));
    }
    let help = &ed.selected_field().help;
    if !help.is_empty() {
        lines.push(Line::styled(help.clone(), dim));
    }
    lines
}

fn path_edit_lines(ed: &PathEditor) -> Vec<Line<'static>> {
    let dim = Style::new().fg(Color::DarkGray);
    let mut lines = vec![match ed.to_path() {
        Ok(path) => Line::from(vec![Span::styled("Result  ", Style::new().fg(Color::Cyan)), Span::raw(path.to_text())]),
        Err(e) => Line::from(vec![Span::styled("Result  ", Style::new().fg(Color::Cyan)), Span::styled(e, Style::new().fg(Color::Red))]),
    }];
    lines.push(Line::default());

    let input_at = |target: Target| ed.input.as_ref().filter(|(_, t)| *t == target).map(|(inp, _)| inp);
    let input_row = |input: &LineInput| input_line("› edit ".into(), input);

    let mut instance = 0;
    for (i, entry) in ed.entries.iter().enumerate() {
        if let Some(input) = input_at(Target::Insert(i)) {
            lines.push(input_row(input));
        }
        if let Some(input) = input_at(Target::Replace(i)) {
            lines.push(input_row(input));
            continue;
        }
        let selected = i == ed.selected && ed.input.is_none();
        let marker = if selected { "› " } else { "  " };
        let line = match entry {
            Entry::Node(node) => Line::from(vec![
                Span::raw(marker),
                Span::styled(format!("{:<13}", format!("{}/{}", node.type_name(), node.sub_type)), dim),
                Span::raw(node.to_text()),
            ]),
            Entry::Separator => {
                instance += 1;
                Line::from(vec![Span::raw(marker), Span::styled(format!("──── instance {instance} ────"), dim)])
            }
        };
        lines.push(if selected { line.reversed() } else { line });
    }
    if let Some(input) = input_at(Target::Insert(ed.entries.len())) {
        lines.push(input_row(input));
    }
    if ed.entries.is_empty() && ed.input.is_none() {
        lines.push(Line::styled("  (no nodes; press a to add one)", dim));
    }

    lines.push(Line::default());
    if let Some(e) = &ed.error {
        lines.push(Line::styled(format!("✗ {e}"), Style::new().fg(Color::Red)));
        lines.push(Line::default());
    }
    lines.push(Line::styled("Node syntax (UEFI spec §10.6; numbers are decimal or 0x hex):", dim));
    lines.extend(SYNTAX_HELP.iter().map(|s| Line::styled(format!("  {s}"), dim)));
    lines
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match &app.mode {
        Mode::Normal => "q quit · / filter · Tab hex · e edit · x hex-edit · a toggle active · D dump · r reload │ ",
        Mode::Filter => "type to filter · Enter keep · Esc clear │ ",
        Mode::PathEdit(ed) if ed.input.is_some() => "type node text · ←→ Home End move · Enter apply · Esc discard │ ",
        Mode::FormEdit(FormEditor { sub: Some((ed, _)), .. }) if ed.input.is_some() => {
            "type node text · ←→ Home End move · Enter apply · Esc discard │ "
        }
        Mode::PathEdit(_) | Mode::FormEdit(FormEditor { sub: Some(_), .. }) => {
            "↑↓ select · Enter edit · a add node · i add instance · d delete · K/J move · s save · Esc cancel │ "
        }
        Mode::FormEdit(ed) if ed.input.is_some() => "type a value · ←→ Home End move · Enter apply · Esc discard │ ",
        Mode::FormEdit(_) => {
            "↑↓ select · Enter/Space edit or toggle · ←→ choose · d delete · K/J move · t text/hex · s save · Esc cancel │ "
        }
        _ => "",
    };
    let line = Line::from(vec![Span::styled(keys, Style::new().fg(Color::DarkGray)), Span::raw(app.status.clone())]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_confirm(frame: &mut Frame, summary: &[String]) {
    let area = frame.area();
    let width = summary.iter().map(|s| s.chars().count()).max().unwrap_or(20).clamp(30, area.width.saturating_sub(4) as usize) as u16 + 4;
    let height = (summary.len() as u16 + 4).min(area.height);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height,
    };
    let mut lines: Vec<Line> = summary.iter().map(|s| Line::from(s.clone())).collect();
    lines.push(Line::default());
    lines.push(Line::from("y write · n cancel".bold()));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Confirm write ").border_style(Style::new().fg(Color::Red))),
        popup,
    );
}
