use super::*;

fn key(platform: Option<&'static str>, thing: &str) -> Option<ComponentKey> {
    Some(ComponentKey {
        platform,
        thing: thing.to_string(),
    })
}

#[test]
fn a_component_name_gives_its_platform_and_the_thing_its_shots_name() {
    assert_eq!(ComponentKey::of("MacRow"), key(Some("mac"), "row"));
    assert_eq!(
        ComponentKey::of("AndroidCastRow"),
        key(Some("android"), "cast-row")
    );
    assert_eq!(
        ComponentKey::of("AndroidQRScanner"),
        key(Some("android"), "qr-scanner")
    );
    assert_eq!(
        ComponentKey::of("MacS3KeyFields"),
        key(Some("mac"), "s3-key-fields")
    );
    assert_eq!(ComponentKey::of("StatusChip"), key(None, "status-chip"));
    assert_eq!(ComponentKey::of("Machine"), key(None, "machine"));
    assert_eq!(ComponentKey::of("index.d.ts"), None);
    assert_eq!(ComponentKey::of("row"), None);
}

#[test]
fn a_component_s_shots_start_with_its_platform_and_thing() {
    assert_eq!(
        ComponentKey::of("MacCheckbox").unwrap().prefix("mac"),
        "mac.component.checkbox."
    );
    assert_eq!(
        ComponentKey::of("Artwork").unwrap().prefix("android"),
        "android.component.artwork."
    );
}
