use super::*;

#[test]
fn routes_cover_the_canvas_tree() {
    assert_eq!(route("/support.js"), Some(Route::Runtime));
    assert_eq!(route("/storage/support.js"), Some(Route::Runtime));
    assert_eq!(
        route("/ds/bae/tokens.css"),
        Some(Route::Design("bae".into(), DesignFile::TokensCss))
    );
    assert_eq!(
        route("/storage/ds/bae/components/bundle.js"),
        Some(Route::Design("bae".into(), DesignFile::BundleJs))
    );
    assert_eq!(route("/ds/bae/tokens.json"), None);
    assert_eq!(
        route("/mac.a.b.dc.html?x=1"),
        Some(Route::Mocks("mac.a.b.dc.html".into()))
    );
    assert_eq!(route("/"), None);
}

#[test]
fn routes_stay_inside_the_mocks() {
    assert_eq!(route("/../../secret.txt"), None);
    assert_eq!(route("//etc/passwd"), None);
}

#[test]
fn the_sf_mono_files_have_a_route_and_nothing_else_under_it_does() {
    assert_eq!(
        route("/fonts/sf-mono/SF-Mono-Regular.otf"),
        Some(Route::Font("SF-Mono-Regular.otf".into()))
    );
    assert_eq!(route("/fonts/sf-mono/secret.txt"), None);
    assert_eq!(route("/fonts/sf-mono/../SF-Mono-Regular.otf"), None);
}
