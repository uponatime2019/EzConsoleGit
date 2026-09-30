//! Terminal colour palette mirroring the Flutter/Tauri Fluent theme.
//!
//! The alpha-composited diff backgrounds of the desktop apps are pre-blended
//! against the surface colour here, because terminals have no alpha.

use ezgit_core::models::AppThemeMode;
use ratatui::style::Color;

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Theme {
    pub surface0: Color,
    pub surface1: Color,
    pub surface2: Color,
    pub surface3: Color,

    pub border: Color,
    pub border_strong: Color,

    pub text: Color,
    pub text_dim: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub text_ghost: Color,
    pub text_on_accent: Color,

    pub accent: Color,
    pub accent_text: Color,
    pub danger: Color,
    pub warning: Color,
    pub syntax_green: Color,
    pub selection: Color,

    pub badge_modified: Color,
    pub badge_added: Color,
    pub badge_deleted: Color,
    pub badge_untracked: Color,
    pub badge_conflict: Color,

    pub diff_add_bg: Color,
    pub diff_add_fg: Color,
    pub diff_del_bg: Color,
    pub diff_del_fg: Color,
    pub diff_hunk_bg: Color,
    pub diff_hunk_fg: Color,
    pub diff_meta_fg: Color,
}

pub const DARK: Theme = Theme {
    surface0: rgb(0x1A1A1A),
    surface1: rgb(0x1E1E1E),
    surface2: rgb(0x252525),
    surface3: rgb(0x2A2A2A),

    border: rgb(0x3A3A3A),
    border_strong: rgb(0x555555),

    text: rgb(0xFFFFFF),
    text_dim: rgb(0xD4D4D4),
    text_secondary: rgb(0xCCCCCC),
    text_muted: rgb(0xAAAAAA),
    text_faint: rgb(0x888888),
    text_ghost: rgb(0x666666),
    text_on_accent: rgb(0xFFFFFF),

    accent: rgb(0x0078D4),
    accent_text: rgb(0x60CDFF),
    danger: rgb(0xFF7B72),
    warning: rgb(0xE0A96D),
    syntax_green: rgb(0xA5D6A7),
    selection: rgb(0x2E3748),

    badge_modified: rgb(0xE5C07B),
    badge_added: rgb(0x98C379),
    badge_deleted: rgb(0xE06C75),
    badge_untracked: rgb(0xC678DD),
    badge_conflict: rgb(0xFF5555),

    diff_add_bg: rgb(0x1E3522),
    diff_add_fg: rgb(0x7EE787),
    diff_del_bg: rgb(0x462523),
    diff_del_fg: rgb(0xFFA198),
    diff_hunk_bg: rgb(0x1F2B3C),
    diff_hunk_fg: rgb(0x79C0FF),
    diff_meta_fg: rgb(0x8B949E),
};

pub const LIGHT: Theme = Theme {
    surface0: rgb(0xFFFFFF),
    surface1: rgb(0xF8F8F8),
    surface2: rgb(0xFFFFFF),
    surface3: rgb(0xF3F3F3),

    border: rgb(0xD4D4D4),
    border_strong: rgb(0xC0C0C0),

    text: rgb(0x1F1F1F),
    text_dim: rgb(0x3B3B3B),
    text_secondary: rgb(0x3B3B3B),
    text_muted: rgb(0x5A5A5A),
    text_faint: rgb(0x6E6E6E),
    text_ghost: rgb(0x8A8A8A),
    text_on_accent: rgb(0xFFFFFF),

    accent: rgb(0x0078D4),
    accent_text: rgb(0x0078D4),
    danger: rgb(0xCF222E),
    warning: rgb(0x9A6700),
    syntax_green: rgb(0x116329),
    selection: rgb(0xDBEAFE),

    badge_modified: rgb(0x9A6700),
    badge_added: rgb(0x1A7F37),
    badge_deleted: rgb(0xCF222E),
    badge_untracked: rgb(0x8250DF),
    badge_conflict: rgb(0xB42318),

    diff_add_bg: rgb(0xE4F7E8),
    diff_add_fg: rgb(0x1A7F37),
    diff_del_bg: rgb(0xFFE7E5),
    diff_del_fg: rgb(0xCF222E),
    diff_hunk_bg: rgb(0xE4EFFC),
    diff_hunk_fg: rgb(0x0969DA),
    diff_meta_fg: rgb(0x6E7781),
};

/// Selects the palette for a theme mode. `System` resolves to the dark palette
/// since a terminal cannot report the OS colour scheme reliably.
pub fn theme_for(mode: AppThemeMode) -> Theme {
    match mode {
        AppThemeMode::Light => LIGHT,
        _ => DARK,
    }
}
