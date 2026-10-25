//! Colours: Catppuccin Mocha names or `#rrggbb` hex.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// The palette is locus-theme's (L1): one Mocha table for every app.
use locus_theme::MOCHA;

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid colour {0:?}: use a Catppuccin Mocha name (e.g. \"blue\") or #rrggbb")]
pub struct ColourError(pub String);

impl Rgb {
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Named Mocha colour; panics never — unknown names fall back to `text`.
    pub fn mocha(name: &str) -> Rgb {
        name.parse().unwrap_or(Rgb(0xcd, 0xd6, 0xf4))
    }

    /// Linear blend towards `other` by `t` in [0,1].
    pub fn blend(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Rgb(mix(self.0, other.0), mix(self.1, other.1), mix(self.2, other.2))
    }
}

impl FromStr for Rgb {
    type Err = ColourError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some((_, (r, g, b))) = MOCHA.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
            return Ok(Rgb(*r, *g, *b));
        }
        let hex = s;
        let h = hex.strip_prefix('#').ok_or_else(|| ColourError(s.to_string()))?;
        if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ColourError(s.to_string()));
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| ColourError(s.to_string()));
        Ok(Rgb(p(0)?, p(2)?, p(4)?))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_hex() {
        assert_eq!("blue".parse::<Rgb>().unwrap(), Rgb(0x89, 0xb4, 0xfa));
        assert_eq!("#89B4FA".parse::<Rgb>().unwrap(), Rgb(0x89, 0xb4, 0xfa));
        assert!("bleu".parse::<Rgb>().is_err());
        assert!("#89b4f".parse::<Rgb>().is_err());
        assert_eq!(Rgb(1, 2, 3).hex(), "#010203");
    }

    #[test]
    fn blend_endpoints() {
        let a = Rgb(0, 0, 0);
        let b = Rgb(255, 255, 255);
        assert_eq!(a.blend(b, 0.0), a);
        assert_eq!(a.blend(b, 1.0), b);
    }
}
