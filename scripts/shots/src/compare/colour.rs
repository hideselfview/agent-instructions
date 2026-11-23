//! Colour: a picture's colours in an element's box, and CIEDE2000 between
//! them.

use super::normalize::Entry;
use super::{DELTA_E_TOLERANCE, EDGE_TOLERANCE, Part, PictureSide, Side, Warning};
use crate::shots::{Bounds, Element, Picture, Role};
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

/// The colour that fills most of a box, counting
/// colours within [`DELTA_E_TOLERANCE`] of each other as one: a background
/// drawn as a glow of many near shades counts as a whole, and a text's one
/// fill never wins over it for being the most frequent single shade. Returns
/// the most frequent colour of the biggest group, and that group's centre.
fn background_of(counts: &HashMap<Rgb, usize>) -> Option<(Rgb, Lab)> {
    groups_of(counts.iter().map(|(colour, count)| (*colour, *count)))
        .into_iter()
        .max_by_key(|group| (group.total, group.colour))
        .map(|group| (group.colour, group.centre))
}

/// Colours within [`DELTA_E_TOLERANCE`] of each other, counted as one.
struct Group {
    /// The most frequent colour of the group, which is its centre.
    colour: Rgb,
    centre: Lab,
    /// The pixels of every colour in the group.
    total: usize,
}

/// The groups of <counts>, the one of the most frequent colour first.
fn groups_of(counts: impl Iterator<Item = (Rgb, usize)>) -> Vec<Group> {
    let mut by_count: Vec<(Rgb, usize)> = counts.collect();
    // The most frequent colour first, ties by colour, so a group's centre is
    // always its most frequent colour and the result never depends on the
    // hash map's order.
    by_count.sort_by_key(|(colour, count)| (std::cmp::Reverse(*count), std::cmp::Reverse(*colour)));
    let mut groups: Vec<Group> = Vec::new();
    for (colour, count) in by_count {
        let lab = Lab::of(colour);
        match groups
            .iter_mut()
            .find(|group| delta_e(group.centre, lab) <= DELTA_E_TOLERANCE)
        {
            Some(group) => group.total += count,
            None => groups.push(Group {
                colour,
                centre: lab,
                total: count,
            }),
        }
    }
    groups.sort_by_key(|group| {
        (
            std::cmp::Reverse(group.total),
            std::cmp::Reverse(group.colour),
        )
    });
    groups
}

fn dot(first: [i32; 3], second: [i32; 3]) -> i32 {
    first.into_iter().zip(second).map(|(a, b)| a * b).sum()
}

/// The share of an element's box that a foreground colour covers to count as
/// a drawn part (a label, an edge, a switch) and not as antialiasing.
const DRAWN_PART_SHARE: f64 = 0.1;

struct Palette {
    background: Rgb,
    foreground: Vec<(Rgb, usize)>,
    /// Every colour of the box, background included, with its pixels.
    colours: Vec<(Rgb, usize)>,
    /// The pixels of the box.
    pixels: usize,
}

impl Palette {
    fn mode(&self) -> Option<(Rgb, usize)> {
        self.foreground
            .iter()
            .copied()
            .max_by_key(|(colour, count)| (*count, *colour))
    }

    /// The foreground colours the element draws as parts, the one with the
    /// most pixels first: each group of colours within [`DELTA_E_TOLERANCE`]
    /// of each other that covers at least [`DRAWN_PART_SHARE`] of the box.
    fn parts(&self) -> Vec<Group> {
        let least = (self.pixels as f64 * DRAWN_PART_SHARE).ceil() as usize;
        let mut groups = groups_of(self.foreground.iter().copied());
        groups.retain(|group| group.total >= least);
        groups
    }

    /// Whether the box draws <colour>, or one within [`DELTA_E_TOLERANCE`] of
    /// it, at any share.
    fn draws(&self, colour: Rgb) -> bool {
        let lab = Lab::of(colour);
        self.colours
            .iter()
            .any(|(drawn, _)| delta_e(Lab::of(*drawn), lab) <= DELTA_E_TOLERANCE)
    }

    /// Follow the dominant foreground colour away from the background to find
    /// the glyph pixels with the highest coverage. Another colour inside the
    /// text box can have more contrast without lying on that blend line.
    fn glyph_colour(&self) -> Option<Rgb> {
        let (mode, count) = self.mode()?;
        let offset = |colour: Rgb| {
            std::array::from_fn(|channel| {
                i32::from(colour[channel]) - i32::from(self.background[channel])
            })
        };
        let dominant = offset(mode);
        let mut strongest = (dot(dominant, dominant), count, mode);
        for &(colour, count) in &self.foreground {
            let candidate = offset(colour);
            let distance = dot(candidate, candidate);
            let key = (distance, count, colour);
            if key <= strongest {
                continue;
            }
            let coverage = f64::from(dot(dominant, candidate)) / f64::from(distance);
            // The dominant colour can be a partial blend of this candidate
            // with the background. Allow one RGB step for 8-bit rounding.
            if coverage >= 0.0
                && dominant.into_iter().zip(candidate).all(|(sample, full)| {
                    (f64::from(sample) - coverage * f64::from(full)).abs() <= 1.0
                })
            {
                strongest = key;
            }
        }
        Some(strongest.2)
    }
}

/// The dominant background and the foreground colours in `bounds`, in points
/// from the picture's top-left.
fn palette(picture: &Picture, scale: f64, bounds: Bounds) -> Option<Palette> {
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
    let (background, lab) = background_of(&counts)?;
    let pixels = counts.values().sum();
    let foreground = counts
        .iter()
        .map(|(colour, count)| (*colour, *count))
        .filter(|(colour, _)| delta_e(lab, Lab::of(*colour)) > FAR_FROM_BACKGROUND)
        .collect();
    Some(Palette {
        background,
        foreground,
        colours: counts.into_iter().collect(),
        pixels,
    })
}

/// `#RRGGBB`.
pub(crate) fn hex(colour: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2])
}

/// The colour warnings of the pairs, measured from each element's picture.
pub(crate) fn pairs(board: &Side, shot: &Side, pairs: &[(Entry, Entry)]) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let differ = |entry: &Entry, part: Part, from: Rgb, to: Rgb| {
        let delta = rgb_delta(from, to);
        (delta > DELTA_E_TOLERANCE).then(|| Warning::Colour {
            entry: entry.clone(),
            part,
            board: from,
            shot: to,
            delta,
        })
    };
    for (b, s) in pairs {
        // A covered element's box holds the covering view's pixels, on the
        // side that has the covering element and on the other, which draws
        // the same view there.
        let hidden = [
            (PictureSide::Board, board.elements, b),
            (PictureSide::Shot, shot.elements, s),
        ]
        .into_iter()
        .find_map(|(side, list, entry)| covering(list, entry).map(|over| (side, entry, over)));
        if let Some((side, entry, over)) = hidden {
            crate::log::note(format_args!(
                "colours of {:?} “{}” skipped: element {over} of the {side:?} list covers it",
                entry.kind, entry.words
            ));
            continue;
        }
        let from = palette(board.picture, board.scale, b.bounds(board.elements));
        let to = palette(shot.picture, shot.scale, shot.elements[s.index].bounds);
        if let (Some(from), Some(to)) = (&from, &to) {
            // A box that fits its glyph tightly is mostly glyph, and a looser
            // box is mostly surface, so one side's majority colour can be the
            // other's glyph. The backgrounds differ when each side's lacks in
            // the other's box.
            if !from.draws(to.background) && !to.draws(from.background) {
                warnings.extend(differ(b, Part::Background, from.background, to.background));
            }
        }
        match b.kind {
            Role::Text => warnings.extend(
                match (
                    from.as_ref().and_then(Palette::glyph_colour),
                    to.as_ref().and_then(Palette::glyph_colour),
                ) {
                    (Some(from), Some(to)) => differ(b, Part::Text, from, to),
                    (None, Some(_)) => {
                        Some(unmeasured(b, PictureSide::Board, &board.elements[b.index]))
                    }
                    (Some(_), None) => {
                        Some(unmeasured(b, PictureSide::Shot, &shot.elements[s.index]))
                    }
                    (None, None) => None,
                },
            ),
            _ => {
                if let (Some(from), Some(to)) = (&from, &to) {
                    warnings.extend(unmatched(b, from, to));
                }
            }
        }
    }
    warnings
}

/// The index of an element drawn after <entry>, and no part of it, that holds
/// the whole of its box: a sheet, popover or scrim over it. A list holds an
/// element after the elements under it, so the box shows the later element's
/// pixels. A list holds each parent before its children (`Shot::problems`).
fn covering(list: &[Element], entry: &Entry) -> Option<usize> {
    let at = entry.bounds(list);
    let own = |index: usize| index == entry.index || entry.folded.contains(&index);
    let last = entry
        .folded
        .iter()
        .fold(entry.index, |last, i| last.max(*i));
    let part_of_entry = |index: usize| {
        let mut at = Some(index);
        while let Some(i) = at {
            if own(i) {
                return true;
            }
            at = list[i].parent;
        }
        false
    };
    let tolerance = EDGE_TOLERANCE;
    ((last + 1)..list.len()).find(|&index| {
        let over = list[index].bounds;
        !part_of_entry(index)
            && over.x <= at.x + tolerance
            && over.y <= at.y + tolerance
            && over.x + over.width >= at.x + at.width - tolerance
            && over.y + over.height >= at.y + at.height - tolerance
    })
}

/// The foreground colours that one of <board> and <shot> draws as a part of
/// the element and the other draws no colour within [`DELTA_E_TOLERANCE`] of,
/// its background included and at any share of its box: a part that covers
/// just under the share on one side still matches, and so does a glyph that
/// the other side's tighter box makes its background.
fn unmatched(entry: &Entry, board: &Palette, shot: &Palette) -> Vec<Warning> {
    let lacking = |drawn: &Palette, other: &Palette, side: PictureSide| {
        drawn
            .parts()
            .into_iter()
            .filter(|part| !other.draws(part.colour))
            .map(|part| Warning::ForegroundColour {
                entry: entry.clone(),
                side,
                colour: part.colour,
            })
            .collect::<Vec<_>>()
    };
    let mut found = lacking(board, shot, PictureSide::Board);
    found.extend(lacking(shot, board, PictureSide::Shot));
    found
}

fn unmeasured(entry: &Entry, side: PictureSide, element: &Element) -> Warning {
    Warning::TextColourUnavailable {
        entry: entry.clone(),
        side,
        recorded: element.text.as_ref().and_then(|style| style.color.clone()),
    }
}

#[cfg(test)]
#[path = "colour_tests.rs"]
mod tests;
