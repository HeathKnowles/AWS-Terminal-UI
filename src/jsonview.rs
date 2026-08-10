use ratatui::prelude::{Color, Line, Span, Style};

/// One row of the navigable output tree view.
#[derive(Debug, Clone)]
pub struct ViewRow {
    pub label: String,
    pub value: String,
    pub drill: Option<serde_json::Value>,
}

/// Flatten a JSON value into a vertical list of navigable rows.
pub fn view_rows(value: &serde_json::Value) -> Vec<ViewRow> {
    let mut out = Vec::new();
    collect_view_rows(value, 0, &mut out);
    out
}

fn collect_view_rows(value: &serde_json::Value, depth: usize, out: &mut Vec<ViewRow>) {
    let indent = "  ".repeat(depth);
    match value {
        serde_json::Value::Array(items) if items.iter().all(|i| i.is_object()) => {
            for (i, item) in items.iter().enumerate() {
                let obj = item.as_object().unwrap();
                out.push(ViewRow {
                    label: format!("{indent}[{i}] {} fields", obj.len()),
                    value: object_summary(obj),
                    drill: Some(item.clone()),
                });
            }
        }
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                out.push(ViewRow {
                    label: format!("{indent}[{i}]"),
                    value: cell_str(item),
                    drill: (item.is_object() || item.is_array()).then(|| item.clone()),
                });
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                out.push(ViewRow {
                    label: format!("{indent}{k}"),
                    value: cell_str(v),
                    drill: (v.is_object() || v.is_array()).then(|| v.clone()),
                });
            }
        }
        scalar => {
            out.push(ViewRow {
                label: indent,
                value: cell_str(scalar),
                drill: None,
            });
        }
    }
}

/// Compact single-line summary of an object's first few scalar fields.
fn object_summary(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    obj.iter()
        .filter(|(_, v)| !v.is_object() && !v.is_array())
        .take(3)
        .map(|(k, v)| format!("{k}={}", cell_str(v)))
        .collect::<Vec<_>>()
        .join("  ")
}

/// One-line "folder block" header for a JSON value, tree-sitter style.
pub fn block_header(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let fields = m
                .keys()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if m.is_empty() {
                "{}".to_string()
            } else if m.len() > 3 {
                format!("{{ {fields}, … }}")
            } else {
                format!("{{ {fields} }}")
            }
        }
        serde_json::Value::Array(a) => format!("[{} items]", a.len()),
        other => cell_str(other),
    }
}

/// Render a JSON scalar; compact nested values.
pub fn cell_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => compact_value(other, 48),
    }
}

fn compact_value(v: &serde_json::Value, budget: usize) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let mut parts: Vec<String> = Vec::new();
            let mut len = 0usize;
            for it in items {
                let p = compact_value(it, budget.saturating_sub(len).max(4) / 2);
                len += p.len() + 2;
                if len > budget {
                    break;
                }
                parts.push(p);
            }
            if items.len() > parts.len() {
                format!("[{} items]", items.len())
            } else {
                format!("[{}]", parts.join(", "))
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let mut parts: Vec<String> = Vec::new();
            let mut len = 1usize;
            for (k, _) in map {
                let kv = k.clone();
                len += kv.len() + 2;
                if len > budget {
                    break;
                }
                parts.push(kv);
            }
            if map.len() > parts.len() {
                format!("{{{}, …}}", parts.join(", "))
            } else {
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
}

/// Minimal JSON syntax highlighter producing styled lines.
pub fn highlight_json(s: &str) -> Vec<Line<'static>> {
    let key_style = Style::default().fg(Color::Cyan);
    let str_style = Style::default().fg(Color::LightGreen);
    let num_style = Style::default().fg(Color::Yellow);
    let word_style = Style::default().fg(Color::Magenta);
    let punct_style = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                let mut buf = String::new();
                buf.push('"');
                let mut escaped = false;
                for c in chars.by_ref() {
                    buf.push(c);
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        break;
                    }
                }
                // A string is a key if the next non-space char is ':'
                let mut look = chars.clone();
                let is_key = look.find(|c| !c.is_whitespace()) == Some(':');
                spans.push(Span::styled(
                    buf,
                    if is_key { key_style } else { str_style },
                ));
            }
            c if c.is_ascii_digit() || c == '-' => {
                let mut buf = String::new();
                buf.push(c);
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || n == '.' || n == 'e' || n == 'E' || n == '+' || n == '-' {
                        buf.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(buf, num_style));
            }
            ':' | ',' | '{' | '}' | '[' | ']' => {
                spans.push(Span::styled(ch.to_string(), punct_style));
            }
            c if c.is_alphabetic() => {
                let mut buf = String::new();
                buf.push(c);
                while let Some(&n) = chars.peek() {
                    if n.is_alphabetic() {
                        buf.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let style = match buf.as_str() {
                    "true" | "false" | "null" => word_style,
                    _ => Style::default().fg(Color::White),
                };
                spans.push(Span::styled(buf, style));
            }
            '\n' => {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            c => {
                spans.push(Span::styled(
                    c.to_string(),
                    Style::default().fg(Color::White),
                ));
            }
        }
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// Word-wrap text into lines; only breaks on whitespace.
pub fn wrap_text(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line_len = 0usize;
    for word in s.split_whitespace() {
        if line_len + word.len() + 1 > width && line_len > 0 {
            out.push('\n');
            line_len = 0;
        }
        if line_len > 0 {
            out.push(' ');
            line_len += 1;
        }
        out.push_str(word);
        line_len += word.len();
    }
    out
}

/// Convenience: pretty-print a JSON value, falling back to the raw string.
pub fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
