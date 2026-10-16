//! COLORREF palettes chosen for contrast with the sampled taskbar background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub up: u32,
    pub down: u32,
    pub neutral: u32,
    pub muted: u32,
}

pub const LIGHT_TEXT: Palette = Palette {
    up: 0x00AAAAFF,
    down: 0x00A0F279,
    neutral: 0x00EEEEEE,
    muted: 0x00CCCCCC,
};
pub const DARK_TEXT: Palette = Palette {
    up: 0x00200090,   // #900020
    down: 0x002A5500, // #00552A
    neutral: 0x00202020,
    muted: 0x00484848,
};

fn luminance(color: u32) -> f64 {
    let linear = |shift: u32| {
        let c = ((color >> shift) & 255u32) as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(0) + 0.7152 * linear(8) + 0.0722 * linear(16)
}

fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

impl Palette {
    fn score(self, background: u32) -> f64 {
        [self.up, self.down, self.neutral, self.muted]
            .into_iter()
            .map(|color| contrast(color, background))
            .fold(f64::INFINITY, f64::min)
    }
}

pub fn choose(samples: &[u32], current: Palette) -> Palette {
    if samples.is_empty() {
        return current;
    }
    let mut samples = samples.to_vec();
    samples.sort_by(|a, b| luminance(*a).total_cmp(&luminance(*b)));
    let background = samples[samples.len() / 2];
    let best = if DARK_TEXT.score(background) > LIGHT_TEXT.score(background) {
        DARK_TEXT
    } else {
        LIGHT_TEXT
    };
    // Ignore small fluctuations near the crossover (acrylic/desktop animations).
    if best.score(background) > current.score(background) * 1.15 {
        best
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pale_taskbar_uses_dark_red_and_green_with_readable_status() {
        for background in [0x00FFFFFF, 0x00D5F5F6, 0x00CEF4EF] {
            let p = choose(&[background; 6], LIGHT_TEXT);
            assert_eq!(p, DARK_TEXT);
            assert!(p.score(background) >= 4.5);
        }
    }
    #[test]
    fn dark_taskbar_uses_light_text_and_adapts_both_directions() {
        assert_eq!(choose(&[0x00202020; 6], DARK_TEXT), LIGHT_TEXT);
        assert!(LIGHT_TEXT.score(0x00202020) >= 4.5);
        assert_eq!(choose(&[], DARK_TEXT), DARK_TEXT);
        assert_eq!(choose(&[0, 0xFFFFFF, 0xFFFFFF], LIGHT_TEXT), DARK_TEXT);
    }
}
