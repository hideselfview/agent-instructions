use super::*;
use crate::lane::Lane;
use crate::shots_cache::{self, Key};

/// A cached run of two scenes whose report holds findings.json: scene `a`
/// has a missing-button finding and a moved-text warning, scene `b` a colour
/// warning.
fn cached_run(scratch: &crate::testing::Scratch) -> Cached {
    let key = Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::MAC,
        environment: "26B131-17C529".to_string(),
        prefix: "mac.".to_string(),
        versions: crate::shots_program::Versions {
            capture: "c".repeat(16),
            comparison: "b".repeat(16),
        },
    };
    let cached = shots_cache::stage(&scratch.places(), &key)
        .unwrap()
        .finish(1, "FAIL x\n")
        .unwrap();
    std::fs::create_dir_all(cached.report()).unwrap();
    std::fs::write(
        cached.findings_json(),
        records(&[
            ("mac.a", "finding", "Shot is missing the button Set up"),
            ("mac.a", "warning", "Title moved 4 pt down in the shot"),
            ("mac.b", "warning", "Title is a different colour"),
        ]),
    )
    .unwrap();
    cached
}

fn records(findings: &[(&str, &str, &str)]) -> String {
    let findings: Vec<_> = findings
        .iter()
        .map(|(scene, level, sentence)| {
            serde_json::json!({"scene": scene, "level": level, "kind": "new-kind", "sentence": sentence})
        })
        .collect();
    serde_json::json!({"version": "new", "commit": "c", "scenes": ["mac.a", "mac.b"], "findings": findings})
        .to_string()
}

/// A program that stands for a changed shots program: it prints <findings>
/// when it is run as `shots recheck --report <the cached run's report>` and
/// fails otherwise.
fn changed_program(scratch: &crate::testing::Scratch, cached: &Cached, findings: &str) -> PathBuf {
    let answer = scratch.dir.join("answer.json");
    std::fs::write(&answer, findings).unwrap();
    let program = scratch.dir.join("changed-builds");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\n[ \"$1 $2 $3 $4\" = \"shots recheck --report {}\" ] || {{ echo \"wrong arguments: $*\" >&2; exit 2; }}\ncat {}\n",
            cached.report().display(),
            answer.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

#[test]
fn a_changed_program_shows_what_it_clears_and_adds_beside_a_cached_run() {
    let scratch = crate::testing::Scratch::new("shots-check-program");
    let cached = cached_run(&scratch);
    let program = changed_program(
        &scratch,
        &cached,
        &records(&[
            ("mac.a", "finding", "Shot is missing the button Set up"),
            ("mac.b", "warning", "Title is a different colour"),
            ("mac.b", "warning", "Subtitle is a different colour"),
        ]),
    );
    assert_eq!(
        compare(&cached, &program).unwrap(),
        "mac.a\n\
         \x20 clears warning: Title moved 4 pt down in the shot\n\
         mac.b\n\
         \x20 adds   warning: Subtitle is a different colour\n\
         1 cleared and 1 added, in 2 scenes of the 2 scenes the run checked\n"
    );
}

#[test]
fn a_program_that_changes_nothing_says_so() {
    let scratch = crate::testing::Scratch::new("shots-check-program-same");
    let cached = cached_run(&scratch);
    let same = std::fs::read_to_string(cached.findings_json()).unwrap();
    let program = changed_program(&scratch, &cached, &same);
    assert_eq!(
        compare(&cached, &program).unwrap(),
        "0 cleared and 0 added, in 0 scenes of the 2 scenes the run checked\n"
    );
}

#[test]
fn a_program_that_fails_is_reported_with_what_it_printed() {
    let scratch = crate::testing::Scratch::new("shots-check-program-fails");
    let cached = cached_run(&scratch);
    let program = scratch.dir.join("failing-builds");
    std::fs::write(
        &program,
        "#!/bin/sh\necho 'the report holds no list' >&2\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = compare(&cached, &program).unwrap_err().text();
    assert!(error.contains("the report holds no list"), "{error}");
}
