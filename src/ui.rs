use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState,
};

use crate::app::{App, Focus, OutputView, Picker};
use crate::jsonview::{highlight_json, wrap_text};
use crate::theme::THEME;

pub fn draw(f: &mut Frame, app: &mut App) {
    // Layout must fit small terminals (e.g. 24 rows). Order matters:
    // Min panes absorb the leftover space so nothing underflows.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_status(f, app, chunks[0]);
    draw_search(f, app, chunks[1]);

    // Middle: left column (results above details) + full-height output right.
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(chunks[2]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(mid[0]);
    draw_results(f, app, left[0]);
    draw_details(f, app, left[1]);
    draw_output(f, app, mid[1]);

    draw_footer(f, app, chunks[3]);

    if let Some(kind) = app.picker {
        draw_picker(f, app, kind);
    }
    if app.param_popup.is_some() {
        draw_param_popup(f, app);
    }
}

fn pane_border(focus: bool) -> BorderType {
    if focus {
        BorderType::Double
    } else {
        BorderType::Rounded
    }
}

fn pane_border_color(focus: bool) -> Color {
    if focus { THEME.focus } else { THEME.border }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let focus_name = match app.focus {
        Focus::Search => "search",
        Focus::Results => "results",
        Focus::Output => "output",
    };
    let status = Line::from(vec![
        Span::styled(
            " aws-tui ",
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled("region ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.env.active_region.clone(),
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({})", app.env.region_source),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  │  "),
        Span::styled("profile ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.env.active_profile.clone(),
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  │  "),
        Span::styled("focus ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            focus_name,
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(status)
            .style(Style::default().bg(Color::Cyan))
            .block(Block::default().style(Style::default().bg(Color::Cyan))),
        area,
    );
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Search;
    let input = Paragraph::new(app.query.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(pane_border(focused))
            .border_style(Style::default().fg(pane_border_color(focused)))
            .title(Span::styled(
                " Search ",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .title(Span::styled(
                " F2 region · F3 profile · Enter run · '>' follow-up ",
                Style::default().fg(Color::DarkGray),
            )),
    );
    f.render_widget(input, area);
    f.set_cursor_position((area.x + app.query.len() as u16 + 1, area.y + 1));
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|(i, _)| {
            let row = &app.index.rows[*i];
            if row.operation.is_empty() {
                ListItem::new(Span::styled(
                    format!("[{}]", row.service),
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:.<20}", row.service),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(row.operation.clone(), Style::default().fg(Color::White)),
                ]))
            }
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(pane_border(focused))
                .border_style(Style::default().fg(pane_border_color(focused)))
                .title(Span::styled(
                    format!(" Results ({}) ", app.results.len()),
                    Style::default()
                        .fg(THEME.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▍");
    let mut st = ListState::default();
    if let Some(sel) = app.selected
        && let Some(pos) = app.results.iter().position(|(i, _)| *i == sel)
    {
        st = ListState::default().with_selected(Some(pos));
    }
    f.render_stateful_widget(list, area, &mut st);
}

fn draw_details(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Results || app.focus == Focus::Search;
    let (cmd_line, docs, params, validation) = match (&app.intent, &app.validation_error) {
        (Some(i), err) => {
            let op = app
                .current_service()
                .and_then(|s| s.ops.iter().find(|o| o.cli_name == i.operation));
            let docs = op.map(|o| o.docs.clone()).unwrap_or_default();
            let params = op.map(|o| o.input.clone()).unwrap_or_default();
            let err = err.clone();
            (i.command_line(&app.aws_bin), docs, params, err)
        }
        _ => (String::new(), String::new(), Vec::new(), None),
    };

    let mut lines: Vec<Line> = Vec::new();
    // Word-wrap the command line so a long command never breaks the layout;
    // continuation lines are indented to align under the command.
    let wrap_w = (area.width.saturating_sub(4)) as usize;
    for (i, wrapped) in wrap_text(&cmd_line, wrap_w).lines().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if i == 0 { "$ " } else { "  " },
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                wrapped.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    match &validation {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("[ERR] {e}"),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[OK]",
                Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD),
            ),
        ])),
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "input params:",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if params.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for p in &params {
            let star = if p.required { "*" } else { " " };
            let ty = if p.is_structure {
                "json".to_string()
            } else {
                p.type_name.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    star,
                    Style::default().fg(if p.required {
                        THEME.warn
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(
                    format!("--{}", p.cli_name),
                    Style::default().fg(if p.required {
                        Color::White
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(format!("  {ty}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    if !docs.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "docs:",
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for wrapped in wrap_text(&docs, 72).lines() {
            lines.push(Line::from(Span::styled(
                wrapped.to_string(),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    let details_lines = lines.len() as u16;
    let details_widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(pane_border(focused))
            .border_style(Style::default().fg(pane_border_color(focused)))
            .title(Span::styled(
                " Details ",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(details_widget, area);
    if details_lines > area.height.saturating_sub(2) {
        let max_scroll = details_lines.saturating_sub(area.height.saturating_sub(2));
        if app.details_scroll > max_scroll {
            app.details_scroll = max_scroll;
        }
        let mut sb =
            ScrollbarState::new(details_lines as usize).position(app.details_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb,
        );
    }
}

fn draw_output(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Output;
    let view_label = if app.view == OutputView::Blocks {
        "tree"
    } else {
        "json"
    };
    let breadcrumb: String = app
        .drill_stack
        .iter()
        .map(|l| l.label.clone())
        .collect::<Vec<_>>()
        .join(" ▸ ");
    let title = format!(
        " Output{} · {view_label}{} ",
        if let Some(rc) = app.last_exit {
            format!("(exit {rc})")
        } else {
            String::new()
        },
        if breadcrumb.is_empty() {
            String::new()
        } else {
            format!(" · {breadcrumb}")
        }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(pane_border(focused))
        .border_style(Style::default().fg(pane_border_color(focused)))
        .title(Span::styled(
            title,
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD),
        ));

    // Json view: full pretty-printed JSON of the current block.
    if app.view == OutputView::Json {
        let pretty = app
            .last_json
            .as_ref()
            .map(crate::jsonview::pretty_json)
            .unwrap_or_else(|| app.output.clone());
        let lines = highlight_json(&pretty);
        let total = lines.len() as u16;
        let out = Paragraph::new(lines)
            .block(block)
            .scroll((app.output_scroll, 0));
        f.render_widget(out, area);
        draw_vscroll(f, app, total, area);
        return;
    }

    // Blocks view: tree-sitter style foldable block list.
    if let Some(rows) = app.current_rows()
        && !rows.is_empty()
    {
        let item_count = rows.len() as u16;
        if app.table_selected.is_none() {
            app.table_selected = Some(0);
        }
        let selected = app.table_selected.unwrap().min(rows.len() - 1);
        app.table_selected = Some(selected);

        let viewport = area.height.saturating_sub(2);
        if (selected as u16) >= (app.output_scroll + viewport) {
            app.output_scroll = (selected as u16).saturating_add(1).saturating_sub(viewport);
        } else if (selected as u16) < app.output_scroll {
            app.output_scroll = selected as u16;
        }

        let fold_open = "▾";
        let fold_closed = "▸";
        // Render the drill path as open tree-sitter folders above the blocks.
        let mut all_items: Vec<ListItem> = app
            .drill_stack
            .iter()
            .enumerate()
            .map(|(i, lvl)| {
                let indent = "    ".repeat(i);
                ListItem::new(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(indent, Style::default()),
                    Span::styled(
                        format!("{fold_open} "),
                        Style::default()
                            .fg(THEME.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{}  {}", lvl.label, lvl.preview),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
                .style(Style::default().bg(Color::Indexed(236)))
            })
            .collect();
        let rows: Vec<ListItem> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let selected = i == selected;
                let is_parent = r.drill.is_some();
                let fold = if is_parent { fold_closed } else { " " };
                let mark = if selected { "▍ " } else { "  " };
                let indent = "    ".repeat(app.drill_stack.len() + 1);
                ListItem::new(Line::from(vec![
                    Span::styled(mark, Style::default().fg(THEME.accent)),
                    Span::styled(indent, Style::default()),
                    Span::styled(
                        format!("{fold} "),
                        Style::default().fg(if is_parent {
                            THEME.accent
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::styled(
                        r.label.clone(),
                        Style::default()
                            .fg(THEME.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        if is_parent {
                            format!(
                                "  {}",
                                crate::jsonview::block_header(&r.drill.clone().unwrap())
                            )
                        } else {
                            format!(": {}", r.value.clone())
                        },
                        Style::default().fg(if selected {
                            Color::Black
                        } else {
                            Color::DarkGray
                        }),
                    ),
                ]))
                .style(if selected {
                    Style::default().bg(THEME.accent)
                } else if i % 2 == 1 {
                    Style::default().bg(Color::Indexed(235))
                } else {
                    Style::default()
                })
            })
            .collect();
        all_items.extend(rows);
        let selected_in_list = app.drill_stack.len() + selected;
        let list = List::new(all_items).block(block).highlight_symbol("");
        let mut st = ListState::default().with_selected(Some(selected_in_list));
        f.render_stateful_widget(list, area, &mut st);
        draw_vscroll(f, app, item_count + app.drill_stack.len() as u16, area);
        return;
    }

    // Fallback: raw / syntax-highlighted JSON
    let lines = highlight_json(&app.output);
    let out = Paragraph::new(lines)
        .block(block)
        .scroll((app.output_scroll, 0));
    f.render_widget(out, area);
    draw_vscroll(f, app, app.output_line_count(), area);
}

fn draw_vscroll(f: &mut Frame, app: &mut App, total: u16, area: Rect) {
    let viewport = area.height.saturating_sub(2);
    if total <= viewport {
        return;
    }
    let max_scroll = total.saturating_sub(viewport);
    if app.output_scroll > max_scroll {
        app.output_scroll = max_scroll;
    }
    let mut sb = ScrollbarState::new(total as usize).position(app.output_scroll as usize);
    f.render_stateful_widget(
        Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
        area,
        &mut sb,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let status_style = match &app.status {
        s if s.starts_with("ok") => Style::default().fg(THEME.ok),
        s if s.starts_with("failed") || s.starts_with("running") => Style::default().fg(THEME.err),
        _ => Style::default().fg(Color::DarkGray),
    };
    let footer = Line::from(vec![
        Span::styled(
            " [F2] region  [F3] profile  [Tab] focus  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "Enter run/drill  ",
            Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[PgUp/PgDn] scroll  [v] tree/json  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "[Esc] back  [q/Ctrl-C] quit",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!(
                "  |  {}{}",
                app.provenance_line,
                if app.status.is_empty() {
                    String::new()
                } else {
                    format!("  |  {}", app.status)
                }
            ),
            status_style,
        ),
    ]);
    f.render_widget(
        Paragraph::new(footer).style(Style::default().bg(Color::Black)),
        area,
    );
}

fn draw_picker(f: &mut Frame, app: &App, kind: Picker) {
    let title = match kind {
        Picker::Region => " Select region ",
        Picker::Profile => " Select profile ",
    };
    let items = app.picker_items().unwrap();
    let pop_w = 40.min(f.area().width.saturating_sub(4));
    let pop_h = (items.len() as u16 + 2).min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let selected = i == app.picker_idx;
            let mark = if selected { "▸ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::default().fg(THEME.accent)),
                Span::styled(
                    name.clone(),
                    Style::default().fg(if selected { Color::Black } else { Color::White }),
                ),
            ]))
        })
        .collect();
    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(THEME.accent))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(THEME.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    let mut st = ListState::default().with_selected(Some(app.picker_idx));
    f.render_widget(Clear, pop);
    f.render_stateful_widget(list, pop, &mut st);
}

fn draw_param_popup(f: &mut Frame, app: &mut App) {
    let Some(popup) = app.param_popup.as_mut() else {
        return;
    };
    let op_label = app
        .intent
        .as_ref()
        .map(|i| format!("{}.{}", i.service, i.operation))
        .unwrap_or_default();
    let title = format!(" Required parameters: {op_label} ");

    let pop_w = 62.min(f.area().width.saturating_sub(4));
    let field_count = popup.fields.len() as u16;
    let pop_h = (field_count + 3).min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let inner_w = pop_w.saturating_sub(2) as usize;
    let value_col = inner_w.saturating_sub(24);

    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in popup.fields.iter().enumerate() {
        let active = i == popup.active;
        let mark = if active { "▍" } else { "  " };
        let ty = &field.type_label;
        let label = format!("{mark}--{:<16} ({ty:<5})", field.cli_name);
        let shown: String = field
            .value
            .chars()
            .take(value_col.saturating_sub(2))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(
                label,
                Style::default().fg(if active {
                    Color::White
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw(" "),
            Span::styled(
                shown,
                Style::default().fg(if active { Color::Black } else { Color::White }),
            ),
        ]));
    }
    match &popup.error {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("! ", Style::default().fg(THEME.err)),
            Span::styled(
                e.clone(),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => lines.push(Line::from(Span::styled(
            "Enter run · Esc cancel · Tab/↑↓ next field",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(THEME.accent))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::DarkGray)),
    );
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);

    if let Some(field) = popup.fields.get(popup.active) {
        let cursor_x = x
            + 1
            + 24
            + 1
            + field
                .value
                .chars()
                .take(value_col.saturating_sub(2))
                .count() as u16;
        f.set_cursor_position((cursor_x, y + 1 + popup.active as u16));
    }
}
