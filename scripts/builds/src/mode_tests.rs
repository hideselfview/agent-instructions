use super::*;
use crate::fail::Fail;
use crate::testing::Scratch;

#[test]
fn mode_defaults_to_strict_and_records_the_switch() {
    let scratch = Scratch::new("mode");
    let tool = scratch.tool(0);
    assert_eq!(current(&tool.places).unwrap(), "strict");
    cmd_mode(&tool, &["quick".into()]).unwrap();
    assert_eq!(current(&tool.places).unwrap(), "quick");
    cmd_mode(&tool, &["strict".into()]).unwrap();
    assert_eq!(current(&tool.places).unwrap(), "strict");
    assert!(matches!(cmd_mode(&tool, &["fast".into()]), Err(Fail::Message(m)) if m.contains("usage")));
}

#[test]
fn an_unrecognized_file_is_an_error_not_a_mode() {
    let scratch = Scratch::new("mode-bad");
    let tool = scratch.tool(0);
    std::fs::write(tool.places.root.join("mode"), "fast\n").unwrap();
    assert!(matches!(current(&tool.places), Err(Fail::Message(m)) if m.contains("not quick or strict")));
}
