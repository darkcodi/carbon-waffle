use iced::{
    Border, Color, Font, Theme, color, font,
    widget::{button, container, pick_list, rule, text_input},
};

pub const BACKGROUND: Color = color!(0x20252b);
pub const SURFACE: Color = color!(0x282e35);
pub const RECESSED: Color = color!(0x1b2025);
pub const LINE: Color = color!(0x3a424b);
pub const FOREGROUND: Color = color!(0xedf1f3);
pub const MUTED: Color = color!(0x9da8b2);
pub const ACCENT: Color = color!(0x85dfa4);
pub const ACCENT_SOFT: Color = color!(0x293d34);
pub const ERROR: Color = color!(0xf29494);
pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

pub fn theme() -> Theme {
    Theme::custom(
        "Carbon",
        iced::theme::Palette {
            background: BACKGROUND,
            text: FOREGROUND,
            primary: ACCENT,
            success: ACCENT,
            warning: color!(0xe1bd7d),
            danger: ERROR,
        },
    )
}

fn border(color: Color, radius: f32) -> Border {
    Border {
        color,
        width: 1.0,
        radius: radius.into(),
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style::default()
        .background(SURFACE)
        .border(border(LINE, 12.0))
}

pub fn console(_: &Theme) -> container::Style {
    container::Style::default()
        .background(RECESSED)
        .border(border(LINE, 10.0))
}

pub fn divider(_: &Theme) -> rule::Style {
    rule::Style {
        color: LINE,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Hovered => (color!(0xa3ebba), BACKGROUND),
        button::Status::Pressed => (color!(0x6ecf91), BACKGROUND),
        button::Status::Disabled => (color!(0x333e39), color!(0x87978d)),
        button::Status::Active => (ACCENT, BACKGROUND),
    };
    button::Style {
        background: Some(background.into()),
        text_color,
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                LINE
            } else {
                SURFACE
            }
            .into(),
        ),
        text_color: if status == button::Status::Disabled {
            MUTED.scale_alpha(0.5)
        } else {
            FOREGROUND
        },
        border: border(LINE, 8.0),
        ..Default::default()
    }
}

pub fn quiet(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => FOREGROUND,
            button::Status::Disabled => MUTED.scale_alpha(0.4),
            button::Status::Active => MUTED,
        },
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = secondary(theme, status);
    style.text_color = if status == button::Status::Disabled {
        MUTED
    } else {
        ERROR
    };
    style
}

pub fn choice(selected: bool, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            if selected {
                ACCENT_SOFT
            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                color!(0x303740)
            } else {
                SURFACE
            }
            .into(),
        ),
        text_color: FOREGROUND,
        border: border(if selected { ACCENT } else { LINE }, 10.0),
        ..Default::default()
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: RECESSED.into(),
        border: border(
            if matches!(status, text_input::Status::Focused { .. }) {
                ACCENT
            } else {
                LINE
            },
            8.0,
        ),
        icon: MUTED,
        placeholder: MUTED,
        value: FOREGROUND,
        selection: ACCENT_SOFT,
    }
}

pub fn select(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    pick_list::Style {
        text_color: FOREGROUND,
        placeholder_color: MUTED,
        handle_color: MUTED,
        background: RECESSED.into(),
        border: border(
            if matches!(status, pick_list::Status::Opened { .. }) {
                ACCENT
            } else {
                LINE
            },
            8.0,
        ),
    }
}
