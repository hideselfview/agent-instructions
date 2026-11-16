use super::*;
use crate::testing::Scratch;

#[test]
fn the_fixed_compile_runs_and_leaves_nothing_behind() {
    let scratch = Scratch::new("core-speed");
    let dir = scratch.dir.join("speed");
    let took = time_compile(&dir).unwrap();
    assert!(took > Duration::ZERO);
    assert!(
        !dir.exists(),
        "time_compile removes its folder when it finishes"
    );
}

#[test]
fn parse_line_reads_a_hosts_time_and_factor_gives_its_speed() {
    let took = Duration::from_millis(1500);
    assert_eq!(parse_line(&line(took)).unwrap(), took);
    assert!(parse_line("ms=0\n").is_err());
    assert!(parse_line("nothing\n").is_err());
    // A host that takes twice as long has half this Mac's speed.
    assert_eq!(
        factor(Duration::from_millis(1000), Duration::from_millis(2000)),
        0.5
    );
}
