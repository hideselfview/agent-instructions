use super::*;

#[test]
fn the_platforms_and_folders_are_those_of_the_shots_declared_lines() {
    let text = "step a mac x\nshots-declared mac app/Shots\n\
                shots-declared android app/shots/\nshots-capture mac run\n";
    assert_eq!(
        declared(text).unwrap(),
        [
            (App::named_for(Platform::Mac), "app/Shots".to_string()),
            (App::named_for(Platform::Android), "app/shots".to_string())
        ]
    );
    assert!(declared("shots-capture mac run\n").unwrap().is_empty());
}

#[test]
fn an_app_lookup_validates_only_its_declarations() {
    let text = "  shots-declared  mac app/Shots/\n\
                shots-declared android app/first\n\
                shots-declared android app/second\n";
    assert_eq!(
        declared_folder(text, "mac").unwrap(),
        Some("app/Shots".to_string())
    );
    assert_eq!(
        declared_folder(text, "android").unwrap_err().why,
        "gives an app a second declare folder"
    );
    assert_eq!(declared_folder(text, "web").unwrap(), None);
    assert!(declared_folder("shots-declared", "mac").is_err());
    assert_eq!(
        declared_folder("shots-declared macos app/Shots", "mac")
            .unwrap_err()
            .why,
        "names no app (an app line or a platform's word gives one)"
    );
}

#[test]
fn a_shots_declared_line_that_the_shots_program_would_refuse_is_an_error() {
    for (line, why) in [
        ("shots-declared", "names no app"),
        (
            "shots-declared ios a",
            "names no app (an app line or a platform's word gives one)",
        ),
        ("shots-declared mac", "names its app and nothing else"),
        (
            "shots-declared mac a b",
            "gives more than one declare folder",
        ),
        ("shots-declared mac ../a", "names a folder outside the repo"),
        ("shots-declared mac /a", "names a folder outside the repo"),
        (
            "shots-declared mac a\nshots-declared mac b",
            "gives an app a second declare folder",
        ),
    ] {
        assert_eq!(declared(line).unwrap_err().why, why, "{line}");
    }
}

#[test]
fn an_app_named_for_a_platform_needs_no_line_and_another_name_needs_one() {
    let apps = Apps::read("step a mac x\napp windows web\n").unwrap();
    assert_eq!(apps.get("mac"), Some(App::named_for(Platform::Mac)));
    assert_eq!(
        apps.get("windows"),
        Some(App {
            name: "windows".to_string(),
            platform: Platform::Web
        })
    );
    assert_eq!(apps.get("linux"), None);
    assert_eq!(
        Apps::read("app mac mac\n").unwrap().get("mac"),
        Some(App::named_for(Platform::Mac))
    );
}

#[test]
fn two_apps_on_one_platform_declare_their_own_folders() {
    let text = "app windows web\napp linux web\n\
                shots-declared windows app/windows\nshots-declared linux app/linux\n";
    let declared = declared(text).unwrap();
    assert_eq!(declared.len(), 2);
    assert_eq!(declared[0].0.platform, declared[1].0.platform);
    assert_eq!(
        declared_folder(text, "linux").unwrap(),
        Some("app/linux".to_string())
    );
}

#[test]
fn an_app_line_that_is_wrong_is_an_error() {
    for (line, why) in [
        ("app", "names no app"),
        (
            "app Windows web",
            "names an app that is not lower-case words joined by '-'",
        ),
        ("app windows", "names its app and nothing else"),
        (
            "app windows ios",
            "names a platform other than mac, android or web",
        ),
        (
            "app windows web x",
            "gives more than an app and its platform",
        ),
        (
            "app mac android",
            "names a platform's word as the name of an app on another platform",
        ),
        (
            "app a web\napp a web",
            "names an app that an earlier line named",
        ),
    ] {
        assert_eq!(Apps::read(line).unwrap_err().why, why, "{line}");
    }
}

#[test]
fn an_app_name_over_the_most_bytes_is_refused_for_its_length() {
    let longest = "a".repeat(APP_NAME_BYTES);
    assert!(Apps::read(&format!("app {longest} web")).is_ok());
    let error = Apps::read(&format!("app {longest}a web")).unwrap_err();
    assert_eq!(error.why, "names an app in more than 32 bytes");
}
