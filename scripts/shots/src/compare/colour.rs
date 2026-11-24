//! Colour: a picture's colours in an element's box, and CIEDE2000 between
//! them.

use super::normalize::Entry;
use super::{DELTA_E_TOLERANCE, EDGE_TOLERANCE, Part, PictureSide, Side, Warning, rgba};
use crate::names::Platform;
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
    rgba(picture, x, y).map(|[r, g, b, _]| [r, g, b])
}

/// The colour that fills most of a box, counting
/// colours within [`DELTA_E_TOLERANCE`] of each other as one: a background
/// drawn as a glow of many near shades counts as a whole, and a text's one
/// fill never wins over it for being the most frequent single shade. Returns
/// the most frequent colour of the biggest group, and that group's centre.
pub(super) fn background_of(counts: &HashMap<Rgb, usize>) -> Option<(Rgb, Lab)> {
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

/// The share of an element's foreground pixels that a glyph colour fills for
/// the glyphs to reach their full coverage: a thin glyph's strokes are less
/// than a pixel wide, so no pixel shows the colour in full, and the pixels
/// nearest it are a few of the many partial coverages.
const FULL_COVERAGE_SHARE: f64 = 0.25;

/// The least share of a drawn colour that a thin glyph's brightest pixel
/// shows. Thin glyphs measured at 0.78 to 0.92; a peak below this is another
/// colour.
const THIN_GLYPH_PEAK: f64 = 0.7;

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

    /// Whether the glyphs reach their full coverage in `glyph`: the pixels
    /// within one step of it in every channel are at least
    /// [`FULL_COVERAGE_SHARE`] of the foreground.
    fn reaches_full_coverage(&self, glyph: Rgb) -> bool {
        let foreground: usize = self.foreground.iter().map(|(_, count)| count).sum();
        let near: usize = self
            .foreground
            .iter()
            .filter(|(colour, _)| colour.iter().zip(glyph).all(|(a, b)| a.abs_diff(b) <= 1))
            .map(|(_, count)| count)
            .sum();
        near as f64 >= foreground as f64 * FULL_COVERAGE_SHARE
    }

    /// Whether <seen> lies on the line from the background to the colour that
    /// is <drawn> from it, [`THIN_GLYPH_PEAK`] of the way or more, within
    /// [`DELTA_E_TOLERANCE`].
    fn is_on_blend(&self, seen: Rgb, drawn: [i32; 3]) -> bool {
        let length = dot(drawn, drawn);
        if length == 0 {
            return false;
        }
        let peak = f64::from(dot(self.offset(seen), drawn)) / f64::from(length);
        if !(THIN_GLYPH_PEAK..=1.0).contains(&peak) {
            return false;
        }
        let expected: Rgb = std::array::from_fn(|channel| {
            (f64::from(self.background[channel]) + peak * f64::from(drawn[channel]))
                .round()
                .clamp(0.0, 255.0) as u8
        });
        rgb_delta(seen, expected) <= DELTA_E_TOLERANCE
    }

    /// How far each channel of `colour` is from the background.
    fn offset(&self, colour: Rgb) -> [i32; 3] {
        std::array::from_fn(|channel| {
            i32::from(colour[channel]) - i32::from(self.background[channel])
        })
    }

    /// Follow the dominant foreground colour away from the background to find
    /// the glyph pixels with the highest coverage. Another colour inside the
    /// text box can have more contrast without lying on that blend line.
    fn glyph_colour(&self) -> Option<Rgb> {
        let (mode, count) = self.mode()?;
        let offset = |colour: Rgb| self.offset(colour);
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
pub(crate) fn pairs(
    platform: Platform,
    board: &Side,
    shot: &Side,
    pairs: &[(Entry, Entry)],
) -> Vec<Warning> {
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
        .find_map(|(side, list, entry)| {
            covering(platform, list, entry).map(|over| (side, entry, over))
        });
        if let Some((side, entry, over)) = hidden {
            crate::log::note(format_args!(
                "colours of {:?} “{}” skipped: element {over} of the {side:?} list covers it",
                entry.kind, entry.words
            ));
            continue;
        }
        let from = palette(board.picture, board.scale, b.bounds(board.elements));
        let to = palette(shot.picture, shot.scale, shot.elements[s.index].bounds);
        if b.kind == Role::Text {
            let board_glyph = from.as_ref().and_then(Palette::glyph_colour);
            match recorded_colour(&board.elements[b.index]) {
                Err(()) => {
                    warnings.push(unmeasured(b, PictureSide::Board, &board.elements[b.index]));
                    continue;
                }
                Ok(Some(recorded))
                    if from
                        .as_ref()
                        .zip(board_glyph)
                        .is_some_and(|(palette, glyph)| {
                            board_glyph_is_dimmed(palette, glyph, recorded)
                        }) =>
                {
                    crate::log::note(format_args!(
                        "colours of Text “{}” skipped: the board draws it dimmer than the colour it records",
                        b.words
                    ));
                    continue;
                }
                Ok(_) => {}
            }
        }
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
                    (Some(drawn), Some(measured))
                        if matches!((&from, &to), (Some(from), Some(to))
                            if is_partial_coverage(to, measured, from, drawn)
                                || is_partial_coverage(from, drawn, to, measured)) =>
                    {
                        None
                    }
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

/// Whether the glyph colour `thin` of `partial` is a glyph drawn in `full` of
/// `whole` that no pixel covers in full: `partial` reaches no full coverage,
/// and `thin` lies on the line from its background to `full`'s colour,
/// [`THIN_GLYPH_PEAK`] of the way or more, within [`DELTA_E_TOLERANCE`].
fn is_partial_coverage(partial: &Palette, thin: Rgb, whole: &Palette, full: Rgb) -> bool {
    !partial.reaches_full_coverage(thin) && partial.is_on_blend(thin, whole.offset(full))
}

/// The colour a board gives a text in its element list: the board's CSS,
/// which is the colour it draws (`#RRGGBBAA`, as `render::read` writes it).
/// None for a text with no recorded colour or a see-through one; an error for
/// a value that is no such colour, which the reader never writes.
fn recorded_colour(element: &Element) -> Result<Option<Rgb>, ()> {
    let Some(recorded) = element
        .text
        .as_ref()
        .and_then(|style| style.color.as_deref())
    else {
        return Ok(None);
    };
    let byte = |at: usize| {
        recorded
            .strip_prefix('#')
            .filter(|digits| digits.len() == 8)
            .and_then(|digits| digits.get(at..at + 2))
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .ok_or(())
    };
    Ok((byte(6)? == 255).then_some([byte(0)?, byte(2)?, byte(4)?]))
}

/// Whether the board draws a text under something: the glyph colour read from
/// <palette> is far from <recorded>, the colour its element records. The board
/// draws the colour it records, so a glyph far dimmer than it, and not a thin
/// glyph's partial coverage of it, is behind a scrim or clipped by a sheet.
fn board_glyph_is_dimmed(palette: &Palette, glyph: Rgb, recorded: Rgb) -> bool {
    rgb_delta(glyph, recorded) > DELTA_E_TOLERANCE
        && !palette.is_on_blend(glyph, palette.offset(recorded))
}

/// The index of an element drawn over <entry>, and no part of it, that
/// overlaps its box by more than [`EDGE_TOLERANCE`] in each direction: a
/// sheet, popover or scrim over it. The box shows that element's pixels where
/// they overlap, and a glyph the overlap cuts has no colour of its own to
/// measure. A list holds an element after the elements under it, and each
/// parent before its children (`Shot::problems`). On Android the reader
/// lists each window (a menu or a dialog) before the page it opens over, the
/// outer window before the windows inside it (`render::read::build`), so
/// a window listed before the page covers the page's texts as well, and the
/// page, lying under every window, covers none of theirs.
fn covering(platform: Platform, list: &[Element], entry: &Entry) -> Option<usize> {
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
    let root = |index: usize| {
        let mut top = index;
        while let Some(parent) = list[top].parent {
            top = parent;
        }
        top
    };
    let tree = root(entry.index);
    // A window of an Android list lies inside the window or page it opens
    // over, which the list holds after it; a tree of the page lies inside no
    // tree before it.
    let is_inside_earlier_tree = |top: usize| {
        let inner = list[top].bounds;
        (0..top).any(|index| {
            let outer = list[index].bounds;
            list[index].parent.is_none()
                && inner.x >= outer.x - tolerance
                && inner.y >= outer.y - tolerance
                && inner.x + inner.width <= outer.x + outer.width + tolerance
                && inner.y + inner.height <= outer.y + outer.height + tolerance
        })
    };
    // Android lists each window before what it opens over, the outer window
    // before the windows inside it: a tree before the entry's covers it unless
    // the entry's own tree is a window inside it, and a tree after covers it
    // only when that tree is a window inside an earlier one.
    let drawn_over = |index: usize| {
        if index > last {
            platform != Platform::Android
                || root(index) == tree
                || is_inside_earlier_tree(root(index))
        } else {
            platform == Platform::Android
                && list[index].parent.is_none()
                && index < tree
                && !is_inside_earlier_tree(tree)
        }
    };
    (0..list.len()).find(|&index| {
        let over = list[index].bounds;
        drawn_over(index)
            && !part_of_entry(index)
            && over.x + over.width > at.x + tolerance
            && over.x < at.x + at.width - tolerance
            && over.y + over.height > at.y + tolerance
            && over.y < at.y + at.height - tolerance
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
