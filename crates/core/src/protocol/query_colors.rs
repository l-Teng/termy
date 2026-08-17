use crate::TerminalColor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalQueryColors {
    pub ansi: [TerminalColor; 16],
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    /// Optional configured cursor color used by Tmon when answering OSC 12.
    pub cursor: Option<TerminalColor>,
}

impl Default for TerminalQueryColors {
    fn default() -> Self {
        Self {
            ansi: [
                TerminalColor {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xcd,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0xcd,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0x00,
                    b: 0xee,
                },
                TerminalColor {
                    r: 0xcd,
                    g: 0x00,
                    b: 0xcd,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xcd,
                    b: 0xcd,
                },
                TerminalColor {
                    r: 0xe5,
                    g: 0xe5,
                    b: 0xe5,
                },
                TerminalColor {
                    r: 0x7f,
                    g: 0x7f,
                    b: 0x7f,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0x00,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xff,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0xff,
                    b: 0x00,
                },
                TerminalColor {
                    r: 0x5c,
                    g: 0x5c,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0x00,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0x00,
                    g: 0xff,
                    b: 0xff,
                },
                TerminalColor {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
            ],
            foreground: TerminalColor {
                r: 0xe5,
                g: 0xe5,
                b: 0xe5,
            },
            background: TerminalColor {
                r: 0x1e,
                g: 0x1e,
                b: 0x1e,
            },
            cursor: None,
        }
    }
}

impl TerminalQueryColors {
    pub(crate) fn indexed_color(self, idx: u8) -> TerminalColor {
        match idx {
            0..=15 => self.ansi[idx as usize],
            16..=231 => {
                let idx = idx - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                let to_component = |value: u8| if value == 0 { 0 } else { 55 + (value * 40) };
                TerminalColor {
                    r: to_component(r),
                    g: to_component(g),
                    b: to_component(b),
                }
            }
            232..=255 => {
                let gray = 8 + ((idx - 232) * 10);
                TerminalColor {
                    r: gray,
                    g: gray,
                    b: gray,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_colors_cover_cube_and_grayscale_fallbacks() {
        let colors = TerminalQueryColors::default();
        assert_eq!(colors.indexed_color(16), TerminalColor { r: 0, g: 0, b: 0 });
        assert_eq!(
            colors.indexed_color(232),
            TerminalColor {
                r: 0x08,
                g: 0x08,
                b: 0x08,
            }
        );
        assert_eq!(
            colors.indexed_color(255),
            TerminalColor {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }
}
