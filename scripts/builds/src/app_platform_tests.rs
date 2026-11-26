use super::*;

const XCODE_26: &str = "Xcode 26.3\nBuild version 17C529\n";
const XCODE_27: &str = "Xcode 27.2\nBuild version 27B52\n";

#[test]
fn the_mac_environment_is_the_macos_build_and_the_xcode_build() {
    assert_eq!(parse_mac("26B131\n", XCODE_26).unwrap(), "26B131-17C529");
    assert_eq!(parse_mac("25G241\n", XCODE_27).unwrap(), "25G241-27B52");
}

#[test]
fn a_mac_environment_needs_a_macos_build_and_an_xcode_build_line() {
    assert!(parse_mac("26B131\n", "Xcode 26.3\n").is_err());
    assert!(parse_mac("26B131\n", "").is_err());
    assert!(parse_mac("", XCODE_26).is_err());
    assert!(parse_mac("two words\n", XCODE_26).is_err());
    assert!(parse_mac("26B131\n", "Build version \n").is_err());
}

#[test]
fn the_android_environment_is_the_os_name_and_the_machine() {
    assert_eq!(parse_uname("Darwin arm64\n").unwrap(), "Darwin-arm64");
    assert_eq!(parse_uname("Linux x86_64\n").unwrap(), "Linux-x86_64");
    assert!(parse_uname("").is_err());
    assert!(parse_uname("Darwin\n").is_err());
    assert!(parse_uname("a b c\n").is_err());
}

#[test]
fn an_environment_that_cannot_be_a_folder_name_is_refused() {
    assert!(parse_uname("Dar/win arm64\n").is_err());
    assert!(parse_mac("26B1/31\n", XCODE_26).is_err());
}

#[test]
fn a_lane_reads_its_environment_from_the_commands_of_its_host() {
    let mut asked = Vec::new();
    let mut mac = |script: &str, _: &str| {
        asked.push(script.to_string());
        Ok(match script {
            "sw_vers -buildVersion" => "26B131\n".to_string(),
            _ => XCODE_26.to_string(),
        })
    };
    assert_eq!(
        of(Lane::MAC).environment(&mut mac).unwrap(),
        "26B131-17C529"
    );
    assert_eq!(asked, ["sw_vers -buildVersion", "xcodebuild -version"]);
    let mut android = |script: &str, _: &str| {
        assert_eq!(script, "uname -sm");
        Ok("Darwin arm64\n".to_string())
    };
    assert_eq!(
        of(Lane::ANDROID).environment(&mut android).unwrap(),
        "Darwin-arm64"
    );
}

#[test]
fn a_command_that_fails_fails_the_read_with_what_it_printed() {
    let mut failing = |_: &str, what: &str| -> Result<String> { fail!("{what} failed: no Xcode") };
    let error = of(Lane::MAC).environment(&mut failing).unwrap_err().text();
    assert!(error.contains("no Xcode"), "{error}");
}

#[test]
fn each_platform_names_what_it_needs_from_the_host() {
    let row = |platform| {
        let host = of(platform);
        (
            host.builds_with_xcode(),
            host.opens_windows(),
            host.install(),
        )
    };
    assert_eq!(row(Lane::MAC), (true, true, Install::CopiedApp));
    assert_eq!(row(Lane::ANDROID), (false, false, Install::OnDevice));
}
