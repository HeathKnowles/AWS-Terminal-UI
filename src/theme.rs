use ratatui::prelude::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub border: Color,
    pub focus: Color,
    pub ok: Color,
    pub err: Color,
    pub warn: Color,
}

pub const THEME: Theme = Theme {
    accent: Color::Cyan,
    border: Color::DarkGray,
    focus: Color::Cyan,
    ok: Color::LightGreen,
    err: Color::LightRed,
    warn: Color::Yellow,
};
