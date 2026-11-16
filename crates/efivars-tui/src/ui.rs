//! Rendering.

use efivars_core::decode::{self, Field};
use efivars_core::{Variable, attrs, guids, hex};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, HexEditor, Mode, OrderEditor, View};

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
        (Mode::OrderEdit(ed), _) => {
            lines.extend(order_edit_lines(app, ed));
            "reorder"
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

fn order_edit_lines(app: &App, ed: &OrderEditor) -> Vec<Line<'static>> {
    ed.order
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let name = format!("Boot{n:04X}");
            let desc = app
                .vars
                .iter()
                .find(|v| v.key.vendor == guids::EFI_GLOBAL_VARIABLE && v.key.name == name)
                .and_then(|v| efivars_core::load_option::LoadOption::parse(&v.data).ok())
                .map(|o| o.description.to_string())
                .unwrap_or_else(|| "(missing)".into());
            let line = Line::from(format!("{i:>2}. {name}  {desc}"));
            if i == ed.selected { line.reversed() } else { line }
        })
        .collect()
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.mode {
        Mode::Normal => "q quit · / filter · Tab hex · e edit · x hex-edit · a toggle active · D dump · r reload │ ",
        Mode::Filter => "type to filter · Enter keep · Esc clear │ ",
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
