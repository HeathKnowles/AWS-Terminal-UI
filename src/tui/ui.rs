use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState,
};

use crate::tui::app::{App, Focus, OutputView, Picker};
use crate::tui::jsonview::{highlight_json, wrap_text};
use crate::tui::theme::THEME;

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
    if app.show_plan {
        draw_plan(f, app);
    }
    if app.show_incident {
        draw_incident(f, app);
    }
    if app.show_history {
        draw_history(f, app);
    }
    if app.show_inbox {
        draw_inbox(f, app);
    }
    if app.danger.is_some() {
        draw_danger(f, app);
    }
    if app.show_help {
        draw_help(f, app);
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
    let mut status = status;
    if app.read_only {
        status.spans.push(Span::raw("  │  "));
        status.spans.push(Span::styled(
            "read-only",
            Style::default()
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ));
    }
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
            (i.masked_command_line(&app.aws_bin), docs, params, err)
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
            .map(crate::tui::jsonview::pretty_json)
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
                                crate::tui::jsonview::block_header(&r.drill.clone().unwrap())
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
            "Enter run/drill [e] params [p] plan [i] incident [h] history [t] tasks [?] help  ",
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
    let title = format!(" Parameters: {op_label} ");

    let total = popup.total_rows() as u16;
    let pop_w = 62.min(f.area().width.saturating_sub(4));
    let pop_h = (total + 3).min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let inner_w = pop_w.saturating_sub(2) as usize;
    let value_col = inner_w.saturating_sub(24);

    let mut lines: Vec<Line> = Vec::new();
    let visible = (pop_h.saturating_sub(3)) as usize;
    let visible = visible.max(1);
    if popup.active < popup.scroll {
        popup.scroll = popup.active;
    } else if popup.active >= popup.scroll + visible {
        popup.scroll = popup.active + 1 - visible;
    }
    let start = popup.scroll.min(popup.total_rows());
    let end = (start + visible).min(popup.total_rows());
    for idx in start..end {
        let is_active = idx == popup.active;
        let mark = if is_active { "▍" } else { "  " };
        if idx < popup.fields.len() {
            let field = &popup.fields[idx];
            let star = if field.required { "*" } else { " " };
            let ty = &field.type_label;
            let label = format!("{mark}{star}--{:<15} ({ty:<5})", field.cli_name);
            let shown: String = field
                .value
                .chars()
                .take(value_col.saturating_sub(2))
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    label,
                    Style::default().fg(if is_active || field.required {
                        Color::White
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(" "),
                Span::styled(
                    shown,
                    Style::default().fg(if is_active {
                        Color::Black
                    } else {
                        Color::White
                    }),
                ),
            ]));
            continue;
        }
        if idx == popup.fields.len() {
            let shown: String = popup
                .query
                .chars()
                .take(value_col.saturating_sub(2))
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{mark} --{:<15} (jmes )", "query"),
                    Style::default().fg(if is_active {
                        Color::White
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(" "),
                Span::styled(
                    shown,
                    Style::default().fg(if is_active {
                        Color::Black
                    } else {
                        Color::White
                    }),
                ),
            ]));
            continue;
        }
        if idx == popup.fields.len() + 1 {
            let shown: String = popup
                .output
                .chars()
                .take(value_col.saturating_sub(2))
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{mark} --{:<15} (enum )", "output"),
                    Style::default().fg(if is_active {
                        Color::White
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(" "),
                Span::styled(
                    shown,
                    Style::default().fg(if is_active {
                        Color::Black
                    } else {
                        Color::White
                    }),
                ),
            ]));
            continue;
        }
        if idx == popup.fields.len() + 2 {
            let shown: String = popup
                .max_items
                .chars()
                .take(value_col.saturating_sub(2))
                .collect();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{mark} --{:<15} (int  )", "max-items"),
                    Style::default().fg(if is_active {
                        Color::White
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(" "),
                Span::styled(
                    shown,
                    Style::default().fg(if is_active {
                        Color::Black
                    } else {
                        Color::White
                    }),
                ),
            ]));
            continue;
        }
        let shown: String = popup
            .page_size
            .chars()
            .take(value_col.saturating_sub(2))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{mark} --{:<15} (int  )", "page-size"),
                Style::default().fg(if is_active {
                    Color::White
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw(" "),
            Span::styled(
                shown,
                Style::default().fg(if is_active {
                    Color::Black
                } else {
                    Color::White
                }),
            ),
        ]));
    }
    // Inline docs for the active param field, so types/enums are explained
    // without leaving the editor.
    if let Some(field) = popup.fields.get(popup.active) {
        let mut doc = String::new();
        if !field.enum_values.is_empty() {
            doc = format!("options: {}", field.enum_values.join(", "));
        } else if !field.docs.trim().is_empty() {
            doc = field.docs.trim().to_string();
        }
        if !doc.is_empty() {
            for wrapped in wrap_text(&doc, inner_w.saturating_sub(4)).lines().take(2) {
                lines.push(Line::from(Span::styled(
                    format!("  {wrapped}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
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
            "Enter run · Esc cancel · Tab/↑↓ field · empty optional skipped",
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

    let visible_idx = popup.active.saturating_sub(popup.scroll);
    let current_len = if popup.active < popup.fields.len() {
        popup.fields[popup.active]
            .value
            .chars()
            .take(value_col.saturating_sub(2))
            .count() as u16
    } else if popup.active == popup.fields.len() {
        popup
            .query
            .chars()
            .take(value_col.saturating_sub(2))
            .count() as u16
    } else if popup.active == popup.fields.len() + 1 {
        popup
            .output
            .chars()
            .take(value_col.saturating_sub(2))
            .count() as u16
    } else if popup.active == popup.fields.len() + 2 {
        popup
            .max_items
            .chars()
            .take(value_col.saturating_sub(2))
            .count() as u16
    } else {
        popup
            .page_size
            .chars()
            .take(value_col.saturating_sub(2))
            .count() as u16
    };
    let cursor_x = x + 1 + 24 + 1 + current_len;
    f.set_cursor_position((cursor_x, y + 1 + visible_idx as u16));
}

fn plan_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(err) = app.plan_error.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("plan ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("[ERR] {err}"),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("source: {}", app.plan_source),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let Some(plan) = app.plan.as_ref() else {
        if app.plan_error.is_none() {
            lines.push(Line::from(Span::styled(
                "no plan loaded",
                Style::default().fg(Color::DarkGray),
            )));
        }
        return lines;
    };
    lines.push(Line::from(Span::styled(
        plan.title.clone(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("target: {}", plan.target),
        Style::default().fg(Color::DarkGray),
    )));
    if app.plan_error.is_none() {
        lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[OK]",
                Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::default());
    for (n, step) in plan.steps.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}. {} ", n + 1, step.id),
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}.{}", step.service, step.operation),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                if step.requires_approval {
                    "  [approval]"
                } else {
                    ""
                },
                Style::default().fg(THEME.warn).add_modifier(Modifier::BOLD),
            ),
        ]));
        for wrapped in wrap_text(&format!("  {}", step.title), width).lines() {
            lines.push(Line::from(Span::styled(
                wrapped.to_string(),
                Style::default().fg(Color::Gray),
            )));
        }
        for (k, v) in &step.params {
            for wrapped in wrap_text(&format!("  --{k} {v}"), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        if let Some(q) = step.query.as_deref() {
            for wrapped in wrap_text(&format!("  --query {q}"), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        for section in [&step.preconditions, &step.expected] {
            for item in section {
                for wrapped in wrap_text(&format!("  - {item}"), width).lines() {
                    lines.push(Line::from(Span::styled(
                        wrapped.to_string(),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }
    }
    for (label, items) in [
        ("risks", &plan.risks),
        ("preconditions", &plan.preconditions),
        ("rollback", &plan.rollback),
        ("expected", &plan.expected),
    ] {
        if items.is_empty() {
            continue;
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("{label}:"),
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for item in items {
            for wrapped in wrap_text(&format!("  - {item}"), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
    }
    lines
}

fn draw_plan(f: &mut Frame, app: &mut App) {
    let pop_w = 72.min(f.area().width.saturating_sub(4));
    let pop_h = (f.area().height.saturating_sub(4)).max(5);
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);
    let wrap_w = (pop_w.saturating_sub(4)) as usize;

    let mut lines = plan_lines(app, wrap_w.max(20));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "read-only · ↑↓/PgUp/PgDn scroll · Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let total = lines.len() as u16;
    let viewport = pop_h.saturating_sub(2);
    let max_scroll = total.saturating_sub(viewport);
    if app.plan_scroll > max_scroll {
        app.plan_scroll = max_scroll;
    }
    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(THEME.accent))
                .title(Span::styled(
                    " Plan (read-only) ",
                    Style::default()
                        .fg(THEME.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(Color::Black)),
        )
        .scroll((app.plan_scroll, 0));
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
    if total > viewport {
        let mut sb = ScrollbarState::new(total as usize).position(app.plan_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            pop,
            &mut sb,
        );
    }
}

fn incident_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(err) = app.incident_error.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("incident ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("[ERR] {err}"),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("source: {}", app.incident_source),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let Some(incident) = app.incident.as_ref() else {
        if app.incident_error.is_none() {
            lines.push(Line::from(Span::styled(
                "no incident loaded",
                Style::default().fg(Color::DarkGray),
            )));
        }
        return lines;
    };
    lines.push(Line::from(Span::styled(
        format!("{} {}", incident.id, incident.title),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "status: {}  severity: {}  account: {}  region: {}",
            incident.status,
            incident.severity.as_deref().unwrap_or("-"),
            if incident.account.is_empty() {
                "-"
            } else {
                &incident.account
            },
            if incident.region.is_empty() {
                "-"
            } else {
                &incident.region
            },
        ),
        Style::default().fg(Color::DarkGray),
    )));
    if app.incident_error.is_none() {
        lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[OK]",
                Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "evidence:",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if incident.signals.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for sig in &incident.signals {
        for wrapped in wrap_text(
            &format!("  {} [{}] {}", sig.ts, sig.source, sig.summary),
            width,
        )
        .lines()
        {
            lines.push(Line::from(Span::styled(
                wrapped.to_string(),
                Style::default().fg(Color::Gray),
            )));
        }
        if !sig.detail.trim().is_empty() {
            for wrapped in wrap_text(&format!("    {}", sig.detail.trim()), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "hypotheses:",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if incident.hypotheses.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for h in &incident.hypotheses {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ({}%)", h.id, h.confidence),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if h.refuted { "  [refuted]" } else { "" },
                Style::default().fg(THEME.warn),
            ),
        ]));
        for wrapped in wrap_text(&format!("  {}", h.statement), width).lines() {
            lines.push(Line::from(Span::styled(
                wrapped.to_string(),
                Style::default().fg(Color::Gray),
            )));
        }
        for sup in &h.supporting {
            for wrapped in wrap_text(&format!("    - {sup}"), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "plan:",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )));
    match incident.plan_path.as_deref() {
        Some(p) => {
            lines.push(Line::from(Span::styled(
                format!("  {p}"),
                Style::default().fg(Color::Gray),
            )));
            if let Some(title) = app.linked_plan_title.as_deref() {
                lines.push(Line::from(Span::styled(
                    format!("  {title} ({} steps, valid)", app.linked_plan_steps),
                    Style::default().fg(THEME.ok),
                )));
            }
            if let Some(err) = app.linked_plan_error.as_deref() {
                lines.push(Line::from(vec![
                    Span::styled("  [ERR] ", Style::default().fg(THEME.err)),
                    Span::styled(err.to_string(), Style::default().fg(THEME.err)),
                ]));
            }
            if app.linked_plan_title.is_none() && app.linked_plan_error.is_none() {
                lines.push(Line::from(Span::styled(
                    "  (open with p to inspect)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        ))),
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "approvals:",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )));
    if incident.approvals.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for a in &incident.approvals {
        let approved = a.decision == "approved";
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", a.step), Style::default().fg(Color::White)),
            Span::styled(
                format!("{} by {}", a.decision, a.approver),
                Style::default().fg(if approved { THEME.ok } else { THEME.err }),
            ),
        ]));
        if !a.note.trim().is_empty() {
            for wrapped in wrap_text(&format!("    {}", a.note.trim()), width).lines() {
                lines.push(Line::from(Span::styled(
                    wrapped.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
    lines
}

fn draw_incident(f: &mut Frame, app: &mut App) {
    let pop_w = 74.min(f.area().width.saturating_sub(4));
    let pop_h = (f.area().height.saturating_sub(4)).max(5);
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);
    let wrap_w = (pop_w.saturating_sub(4)) as usize;

    let mut lines = incident_lines(app, wrap_w.max(20));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "read-only · ↑↓/PgUp/PgDn scroll · Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let total = lines.len() as u16;
    let viewport = pop_h.saturating_sub(2);
    let max_scroll = total.saturating_sub(viewport);
    if app.incident_scroll > max_scroll {
        app.incident_scroll = max_scroll;
    }
    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(THEME.accent))
                .title(Span::styled(
                    " Incident workspace (read-only) ",
                    Style::default()
                        .fg(THEME.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(Color::Black)),
        )
        .scroll((app.incident_scroll, 0));
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
    if total > viewport {
        let mut sb = ScrollbarState::new(total as usize).position(app.incident_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            pop,
            &mut sb,
        );
    }
}

fn draw_history(f: &mut Frame, app: &mut App) {
    let pop_w = 74.min(f.area().width.saturating_sub(4));
    let pop_h = (f.area().height.saturating_sub(4)).max(5);
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let visible = (pop_h.saturating_sub(3)) as usize;
    let visible = visible.max(1);
    if !app.history_items.is_empty() {
        app.history_idx = app.history_idx.min(app.history_items.len() - 1);
        if app.history_idx < app.history_scroll {
            app.history_scroll = app.history_idx;
        } else if app.history_idx >= app.history_scroll + visible {
            app.history_scroll = app.history_idx + 1 - visible;
        }
    }
    let start = app.history_scroll.min(app.history_items.len());
    let end = (start + visible).min(app.history_items.len());

    let mut lines: Vec<Line> = Vec::new();
    if app.history_items.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no history yet — run a command first)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (pos, item) in app.history_items.iter().enumerate().take(end).skip(start) {
        let selected = pos == app.history_idx;
        let mark = if selected { "▍ " } else { "  " };
        let exit = match item.exit {
            Some(0) => "ok".to_string(),
            Some(c) => format!("exit {c}"),
            None => "proposed".to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled(mark, Style::default().fg(THEME.accent)),
            Span::styled(
                format!("#{} {}.{} ", item.seq, item.service, item.operation),
                Style::default().fg(if selected { Color::Black } else { Color::White }),
            ),
            Span::styled(
                format!("{exit} {}", item.ts),
                Style::default().fg(if selected {
                    Color::Black
                } else {
                    Color::DarkGray
                }),
            ),
        ]));
    }
    match app.history_error.as_deref() {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("! ", Style::default().fg(THEME.err)),
            Span::styled(
                e.to_string(),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => lines.push(Line::from(Span::styled(
            "Enter rerun (re-validated) · ↑↓ move · Esc close",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    let list_len = lines.len() as u16;
    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(THEME.accent))
            .title(Span::styled(
                " History (rerun re-validates) ",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Black)),
    );
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
    let viewport = pop_h.saturating_sub(2);
    if list_len > viewport {
        let mut sb = ScrollbarState::new(list_len as usize).position(app.history_scroll);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            pop,
            &mut sb,
        );
    }
}

fn draw_danger(f: &mut Frame, app: &mut App) {
    let Some(danger) = app.danger.as_ref() else {
        return;
    };
    let pop_w = 64.min(f.area().width.saturating_sub(4));
    let pop_h = 8.min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);
    let wrap_w = (pop_w.saturating_sub(4)) as usize;

    let mut lines: Vec<Line> = Vec::new();
    for wrapped in wrap_text(&danger.prompt, wrap_w.max(20)).lines().take(2) {
        lines.push(Line::from(Span::styled(
            wrapped.to_string(),
            Style::default().fg(THEME.warn).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("type: ", Style::default().fg(Color::DarkGray)),
        Span::styled(danger.text.clone(), Style::default().fg(Color::White)),
    ]));
    match danger.error.as_deref() {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("! ", Style::default().fg(THEME.err)),
            Span::styled(
                e.to_string(),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => lines.push(Line::from(Span::styled(
            "Enter confirm · Esc cancel",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(THEME.warn))
            .title(Span::styled(
                " Confirm production mutation ",
                Style::default().fg(THEME.warn).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Black)),
    );
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
    f.set_cursor_position((x + 7 + danger.text.len() as u16, y + 3));
}

const HELP_ROWS: &[(&str, &str)] = &[
    ("F2 / F3", "region / profile picker"),
    ("Tab", "cycle focus: search → results → output"),
    ("Enter", "run operation (or follow-up query / drill)"),
    ("e", "edit params, --query, --output, paging (results)"),
    ("p", "open human-written plan, read-only (results)"),
    ("i", "open incident workspace, read-only (results)"),
    ("h", "open command history, Enter reruns (results)"),
    (
        "t",
        "open task inbox, Enter loads, a advances status (results)",
    ),
    ("x", "cancel a running command"),
    ("s", "save output to aws-tui-output-<ts>.json (output)"),
    ("v", "toggle tree / JSON output (output)"),
    ("↑↓ / PgUp/PgDn", "move, scroll output, or scroll overlay"),
    ("Esc", "close overlay → drill up → focus search → clear"),
    ("? (this)", "help is read-only; nothing here executes"),
    ("q / Ctrl-C", "quit (from empty search / anywhere)"),
];

fn draw_help(f: &mut Frame, app: &mut App) {
    let pop_w = 64.min(f.area().width.saturating_sub(4));
    let pop_h = ((HELP_ROWS.len() + 4) as u16).min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let mut lines: Vec<Line> = HELP_ROWS
        .iter()
        .map(|(key, action)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<16}"),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(action.to_string(), Style::default().fg(Color::Gray)),
            ])
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "↑↓ scroll · Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let total = lines.len() as u16;
    let viewport = pop_h.saturating_sub(2);
    let max_scroll = total.saturating_sub(viewport);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(THEME.accent))
                .title(Span::styled(
                    " Keys (read-only) ",
                    Style::default()
                        .fg(THEME.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(Color::Black)),
        )
        .scroll((app.help_scroll, 0));
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
}

fn draw_inbox(f: &mut Frame, app: &mut App) {
    let pop_w = 74.min(f.area().width.saturating_sub(4));
    let pop_h = (f.area().height.saturating_sub(4)).max(5);
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let visible = (pop_h.saturating_sub(3)) as usize;
    let visible = visible.max(1);
    if !app.inbox_items.is_empty() {
        app.inbox_idx = app.inbox_idx.min(app.inbox_items.len() - 1);
        if app.inbox_idx < app.inbox_scroll {
            app.inbox_scroll = app.inbox_idx;
        } else if app.inbox_idx >= app.inbox_scroll + visible {
            app.inbox_scroll = app.inbox_idx + 1 - visible;
        }
    }
    let start = app.inbox_scroll.min(app.inbox_items.len());
    let end = (start + visible).min(app.inbox_items.len());

    let mut lines: Vec<Line> = Vec::new();
    if app.inbox_items.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no incidents stored)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (pos, item) in app.inbox_items.iter().enumerate().take(end).skip(start) {
        let selected = pos == app.inbox_idx;
        let mark = if selected { "▍ " } else { "  " };
        let sev = item.severity.as_deref().unwrap_or("-");
        lines.push(Line::from(vec![
            Span::styled(mark, Style::default().fg(THEME.accent)),
            Span::styled(
                format!("{} ", item.id),
                Style::default().fg(if selected { Color::Black } else { Color::White }),
            ),
            Span::styled(
                format!("{} {sev} ", item.status),
                Style::default().fg(if selected {
                    Color::Black
                } else {
                    Color::DarkGray
                }),
            ),
            Span::styled(
                item.title.clone(),
                Style::default().fg(if selected { Color::Black } else { Color::Gray }),
            ),
        ]));
    }
    match app.inbox_error.as_deref() {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("! ", Style::default().fg(THEME.err)),
            Span::styled(
                e.to_string(),
                Style::default().fg(THEME.err).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => lines.push(Line::from(Span::styled(
            "Enter load workspace · a advance status · Esc close (read-only)",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    let list_len = lines.len() as u16;
    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(THEME.accent))
            .title(Span::styled(
                " Task inbox (stored incidents) ",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Black)),
    );
    f.render_widget(Clear, pop);
    f.render_widget(widget, pop);
    let viewport = pop_h.saturating_sub(2);
    if list_len > viewport {
        let mut sb = ScrollbarState::new(list_len as usize).position(app.inbox_scroll);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            pop,
            &mut sb,
        );
    }
}
