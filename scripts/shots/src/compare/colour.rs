//! Colour: a picture's colours in an element's box, and CIEDE2000 between
//! them.

use super::normalize::Entry;
use super::{DELTA_E_TOLERANCE, Part, Side, Warning};
use crate::shots::{Bounds, Picture};
use std::collections::HashMap;

pub(crate) type Rgb = [u8; 3];

/// CIE L*a*b* under D65.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Lab {
    pub(crate) l: f64,
    pub(crate) a: f64,
    pub(crate) b: f64,
}

fn linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl Lab {
    pub(crate) fn of(rgb: Rgb) -> Self {
        let (r, g, b) = (linear(rgb[0]), linear(rgb[1]), linear(rgb[2]));
        let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
        let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
        let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
        let f = |t: f64| {
            if t > 216.0 / 24389.0 {
                t.cbrt()
            } else {
                (24389.0 / 27.0 * t + 16.0) / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        Self {
            l: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }
}

/// The CIEDE2000 difference between two colours.
pub(crate) fn delta_e(first: Lab, second: Lab) -> f64 {
    let rad = f64::to_radians;
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let c1 = first.a.hypot(first.b);
    let c2 = second.a.hypot(second.b);
    let mean_c = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (mean_c.powi(7) / (mean_c.powi(7) + 25f64.powi(7))).sqrt());
    let (a1, a2) = ((1.0 + g) * first.a, (1.0 + g) * second.a);
    let (c1, c2) = (a1.hypot(first.b), a2.hypot(second.b));
    let (h1, h2) = (hue(first.b, a1), hue(second.b, a2));
    let dl = second.l - first.l;
    let dc = c2 - c1;
    let dh_angle = if c1 * c2 == 0.0 {
        0.0
    } else if (h2 - h1).abs() <= 180.0 {
        h2 - h1
    } else if h2 - h1 > 180.0 {
        h2 - h1 - 360.0
    } else {
        h2 - h1 + 360.0
    };
    let dh = 2.0 * (c1 * c2).sqrt() * rad(dh_angle / 2.0).sin();
    let mean_l = (first.l + second.l) / 2.0;
    let mean_c = (c1 + c2) / 2.0;
    let mean_h = if c1 * c2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * rad(mean_h - 30.0).cos()
        + 0.24 * rad(2.0 * mean_h).cos()
        + 0.32 * rad(3.0 * mean_h + 6.0).cos()
        - 0.20 * rad(4.0 * mean_h - 63.0).cos();
    let shift = 30.0 * (-((mean_h - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (mean_c.powi(7) / (mean_c.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (mean_l - 50.0).powi(2) / (20.0 + (mean_l - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * mean_c;
    let sh = 1.0 + 0.015 * mean_c * t;
    let rt = -rad(2.0 * shift).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh / sh).powi(2) + rt * (dc / sc) * (dh / sh)).sqrt()
}

/// The difference between two sRGB colours.
pub(crate) fn rgb_delta(first: Rgb, second: Rgb) -> f64 {
    delta_e(Lab::of(first), Lab::of(second))
}

/// How far from the background a colour must be to count as foreground.
pub(crate) const FAR_FROM_BACKGROUND: f64 = 10.0;

/// The pixel at (`x`, `y`), when the picture has it.
pub(crate) fn pixel(picture: &Picture, x: i64, y: i64) -> Option<Rgb> {
    if x < 0 || y < 0 || x >= i64::from(picture.width) || y >= i64::from(picture.height) {
        return None;
    }
    let at = (y as usize * picture.width as usize + x as usize) * 4;
    Some([
        picture.pixels[at],
        picture.pixels[at + 1],
        picture.pixels[at + 2],
    ])
}

/// The colour that fills most of a box, counting colours within
/// [`DELTA_E_TOLERANCE`] of each other as one: a background drawn as a glow
/// of many near shades counts as a whole, and a text's one fill never wins
/// over it for being the most frequent single shade. Returns the most
/// frequent colour of the biggest group, and that group's centre.
fn biggest_group(counts: &HashMap<Rgb, usize>) -> Option<(Rgb, Lab)> {
    let mut by_count: Vec<(Rgb, usize)> = counts.iter().map(|(c, n)| (*c, *n)).collect();
    // The most frequent colour first, ties by colour, so a group's centre is
    // always its most frequent colour and the result never depends on the
    // hash map's order.
    by_count.sort_by_key(|(colour, count)| (std::cmp::Reverse(*count), std::cmp::Reverse(*colour)));
    let mut groups: Vec<(Rgb, Lab, usize)> = Vec::new();
    for (colour, count) in by_count {
        let lab = Lab::of(colour);
        match groups
            .iter_mut()
            .find(|(_, centre, _)| delta_e(*centre, lab) <= DELTA_E_TOLERANCE)
        {
            Some(group) => group.2 += count,
            None => groups.push((colour, lab, count)),
        }
    }
    groups
        .into_iter()
        .max_by_key(|(colour, _, total)| (*total, *colour))
        .map(|(colour, lab, _)| (colour, lab))
}

/// The background and foreground of `bounds` (points from the picture's
/// top-left, `scale` pixels to the point): the colour that fills most of the
/// box, and the one that fills most of what is far from it, each counted with
/// its near shades ([`biggest_group`]). The foreground is absent when the box
/// holds one colour.
pub(crate) fn palette(picture: &Picture, scale: f64, bounds: Bounds) -> Option<(Rgb, Option<Rgb>)> {
    let (left, top) = (
        (bounds.x * scale).round() as i64,
        (bounds.y * scale).round() as i64,
    );
    let right = ((bounds.x + bounds.width) * scale).round() as i64;
    let bottom = ((bounds.y + bounds.height) * scale).round() as i64;
    let mut counts: HashMap<Rgb, usize> = HashMap::new();
    for y in top..bottom {
        for x in left..right {
            if let Some(colour) = pixel(picture, x, y) {
                *counts.entry(colour).or_default() += 1;
            }
        }
    }
    let (background, lab) = biggest_group(&counts)?;
    let far: HashMap<Rgb, usize> = counts
        .into_iter()
        .filter(|(colour, _)| delta_e(lab, Lab::of(*colour)) > FAR_FROM_BACKGROUND)
        .collect();
    Some((background, biggest_group(&far).map(|(colour, _)| colour)))
}

/// `#RRGGBB`.
pub(crate) fn hex(colour: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2])
}

/// The colour of a style's `#RRGGBBAA` (or `#RRGGBB`), alpha dropped.
pub(crate) fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.strip_prefix('#')?;
    if !(digits.len() == 6 || digits.len() == 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// The colour warnings of the pairs: each element's background and
/// foreground in its own picture, and a text's colour from its style where
/// both sides give one.
pub(crate) fn pairs(board: &Side, shot: &Side, pairs: &[(Entry, Entry)]) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let mut differ = |entry: &Entry, part: Part, from: Rgb, to: Rgb| {
        let delta = rgb_delta(from, to);
        if delta > DELTA_E_TOLERANCE {
            warnings.push(Warning::Colour {
                entry: entry.clone(),
                part,
                board: from,
                shot: to,
                delta,
            });
        }
    };
    for (b, s) in pairs {
        let (from, to) = (&board.elements[b.index], &shot.elements[s.index]);
        if let (Some((from_back, from_fore)), Some((to_back, to_fore))) = (
            palette(board.picture, board.scale, b.bounds(board.elements)),
            palette(shot.picture, shot.scale, to.bounds),
        ) {
            differ(b, Part::Background, from_back, to_back);
            if let (Some(from_fore), Some(to_fore)) = (from_fore, to_fore) {
                differ(b, Part::Foreground, from_fore, to_fore);
            }
        }
        let styled = |e: &crate::shots::Element| {
            e.text
                .as_ref()
                .and_then(|t| t.color.as_deref())
                .and_then(parse_hex)
        };
        if let (Some(from), Some(to)) = (styled(from), styled(to)) {
            differ(b, Part::Text, from, to);
        }
    }
    warnings
}

#[cfg(test)]
#[path = "colour_tests.rs"]
mod tests;
