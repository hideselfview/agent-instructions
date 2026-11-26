//! The shots prefixes of a design branch, picked from what it changes.
//!
//! `builds app-build <repo> <ref> --design-shots` runs the shots of each
//! board and each component the branch changes, so an agent lists no prefix
//! by hand. A board's shot is named for the board (`mac.welcome.add-code` is
//! design/mocks/mac.welcome.add-code.dc.html). A component's shots are its
//! preview's rows, named `<platform>.component.<thing>.<state>`
//! (`boards::component_key`), on its own platform, or on every lane that
//! declares them for a component every platform shares. A change that names no
//! declared shot gets no pick, and the build says which and why.

use crate::lane::Lane;
use crate::lists::design_paths::{Globs, PlaceKind, classify};
use crate::shots_declared::ShotNames;
use boards::component_key::ComponentKey;

/// The prefixes a branch's changes pick, and what the picks leave out.
#[derive(Debug, PartialEq, Eq)]
pub struct Picked {
    /// Sorted, without repeats.
    pub prefixes: Vec<String>,
    /// Each change that picked no shot, said in a line.
    pub left_out: Vec<String>,
}

/// The prefixes the design files among <changed>, by <globs>, pick, given
/// the shot names each lane declares.
pub fn picked(
    globs: &Globs,
    changed: &[String],
    declared: &mut dyn FnMut(Lane) -> Option<ShotNames>,
) -> Picked {
    // A README changes no drawing (`builds help lists`, design record).
    let drawn: Vec<String> = changed
        .iter()
        .filter(|path| path.rsplit('/').next() != Some("README.md"))
        .cloned()
        .collect();
    let changes = classify(globs, &drawn);
    let mut prefixes = Vec::new();
    let mut left_out = Vec::new();
    let mut declares = |lane: Lane, start: &str| {
        declared(lane).is_some_and(|names| names.iter().any(|name| name.starts_with(start)))
    };
    for place in &changes.places {
        match PlaceKind::of(place) {
            PlaceKind::Board => {
                let scene = place
                    .rsplit('/')
                    .next()
                    .and_then(|file| file.strip_suffix(".dc.html"));
                let lane = scene.and_then(|scene| Lane::parse(scene.split('.').next()?));
                match (scene, lane) {
                    (Some(scene), Some(lane)) if declares(lane, scene) => {
                        prefixes.push(scene.to_string());
                    }
                    _ => left_out.push(format!("the board {place} has no declared shot")),
                }
            }
            PlaceKind::Component => {
                let name = place.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                let Some(key) = ComponentKey::of(name) else {
                    left_out.push(format!("{place} is no component's folder"));
                    continue;
                };
                let lanes: Vec<Lane> = Lane::ALL
                    .into_iter()
                    .filter(|lane| key.platform.is_none_or(|own| own == lane.word()))
                    .collect();
                let before = prefixes.len();
                for lane in lanes {
                    let prefix = key.prefix(lane.word());
                    if declares(lane, &prefix) {
                        prefixes.push(prefix);
                    }
                }
                if prefixes.len() == before {
                    left_out.push(format!("the component {name} has no declared shot"));
                }
            }
            PlaceKind::CanvasIndex | PlaceKind::Spec => {}
        }
    }
    if !changes.sources.is_empty() {
        left_out.push(
            "a shared design-system source changed, and it can change any component's drawing: name the shots of the components it affects with --shots"
                .to_string(),
        );
    }
    prefixes.sort();
    prefixes.dedup();
    Picked { prefixes, left_out }
}

#[cfg(test)]
#[path = "app_design_shots_tests.rs"]
mod tests;
