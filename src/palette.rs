//! Fixed COLORREF text colors for the white ticker background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub up: u32,
    pub down: u32,
    pub neutral: u32,
    pub muted: u32,
}

pub const WHITE_BACKGROUND: Palette = Palette {
    up: 0x000000CC,   // #CC0000
    down: 0x00008000, // #008000
    neutral: 0x00202020,
    muted: 0x00484848,
};
