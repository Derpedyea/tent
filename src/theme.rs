//! Colors. Terminal ANSI palette is One Dark-derived; chrome uses a warm
//! "campfire" accent.

use gpui::{Hsla, Rgba, rgb};
use libghostty_vt::style::RgbColor;

pub const FONT: &str = "JetBrains Mono";

pub const BG: u32 = 0x0f1218;
pub const FG: u32 = 0xd7dce5;
pub const SURFACE: u32 = 0x161a22;
pub const SURFACE_HI: u32 = 0x1d222c;
pub const BORDER: u32 = 0x262c38;
pub const MUTED: u32 = 0x7a8394;
pub const ACCENT: u32 = 0xf0a35e;
pub const OK: u32 = 0x98c379;
pub const WARN: u32 = 0xe5c07b;
pub const ERR: u32 = 0xe06c75;

pub const ANSI: [u32; 16] = [
    0x1c2028, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc8ccd4, //
    0x4b5263, 0xef7a84, 0xa9d68a, 0xf0cf8e, 0x7cc0ff, 0xd595ec, 0x6ccad6, 0xf2f4f8,
];

pub fn rgbc(hex: u32) -> RgbColor {
    RgbColor { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

pub fn hsla(c: RgbColor) -> Hsla {
    Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: 1. }.into()
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}
