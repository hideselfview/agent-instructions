use super::*;

#[test]
fn only_real_dates_parse() {
    assert_eq!(parse("2026-11-13"), Some((2026, 11, 13)));
    assert_eq!(parse("2024-02-29"), Some((2024, 2, 29)));
    for bad in [
        "2026-02-29",
        "2026-13-01",
        "2026-00-10",
        "2026-11-31",
        "2026-1-13",
        "26-11-13",
        "2026/11/13",
        "2026-11-1x",
        "",
    ] {
        assert_eq!(parse(bad), None, "{bad}");
    }
    assert_eq!(parse("1900-02-29"), None);
    assert_eq!(parse("2000-02-29"), Some((2000, 2, 29)));
}
