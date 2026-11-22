use super::*;
use crate::app_timing::{RunRecord, StepKind, StepRecord, StepResult, record};
use crate::lane::Lane;
use crate::lists::fixtures::make_store_named;
use crate::run_end::RunEnd;
use crate::shots_cache::{Key, stage};
use crate::testing::{Scratch, git, git_out, repo};

const BAE: &str = "bae";

/// Keeps a run of <commit> and <prefix> by <capture> and <comparison>, the
/// versions of the shots checker that took its shots and compared them.
fn cache_by(
    places: &Places,
    commit: &str,
    prefix: &str,
    capture: &str,
    comparison: &str,
) -> PathBuf {
    let key = Key {
        repo: BAE.to_string(),
        commit: commit.to_string(),
        lane: Lane::Mac,
        environment: "26B131-17C529".to_string(),
        prefix: prefix.to_string(),
        versions: crate::shots_program::Versions {
            capture: capture.to_string(),
            comparison: comparison.to_string(),
        },
    };
    let staging = stage(places, &key).unwrap();
    std::fs::create_dir_all(staging.report()).unwrap();
    staging.finish(0, "ok\n").unwrap().folder
}

/// Keeps a run by this builds' capture version and <comparison>.
fn cache(places: &Places, commit: &str, prefix: &str, comparison: &str) -> PathBuf {
    cache_by(
        places,
        commit,
        prefix,
        &crate::shots_program::VERSIONS.capture,
        comparison,
    )
}

fn build(branch: &str, commit: &str, prefix: &str) -> RunRecord {
    RunRecord {
        started_at: 4_000_000_000,
        repo: BAE.to_string(),
        lane: Lane::Mac,
        checkout: "side-1".to_string(),
        built_ref: branch.to_string(),
        built_commit: Some(commit.to_string()),
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            at_merge_base: None,
            name: format!("shots {prefix}"),
            kind: StepKind::Shots,
            started_at: 4_000_000_000,
            wall_ms: 1,
            cpu_ms: 1,
            result: StepResult::Passed,
        }],
    }
}

/// A branch with a commit origin/main lacks is waiting to land, and the run
/// that covers its build and the run of the build's merge base are named; a
/// branch whose commit landed, a run of another capture version and a run of
/// another prefix are not. A run of another comparison version is named, since
/// the next lookup compares its pictures again (shots_compare_again.rs).
#[test]
fn a_waiting_branchs_builds_name_their_covering_runs_and_their_base_runs() {
    let scratch = Scratch::new("in-use-named");
    let places = scratch.places();
    let source = scratch.repo_path(BAE);
    repo(&source, "");
    git(&source, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let base = git_out(&source, &["rev-parse", "HEAD"]);
    git(&source, &["switch", "-q", "-c", "waiting"]);
    std::fs::write(source.join("waits"), "w").unwrap();
    git(&source, &["add", "waits"]);
    git(&source, &["commit", "-q", "-m", "Waits"]);
    let waiting = git_out(&source, &["rev-parse", "HEAD"]);
    git(&source, &["switch", "-q", "main"]);
    git(&source, &["switch", "-q", "-c", "landed"]);
    std::fs::write(source.join("landed"), "l").unwrap();
    git(&source, &["add", "landed"]);
    git(&source, &["commit", "-q", "-m", "Landed"]);
    let landed = git_out(&source, &["rev-parse", "HEAD"]);
    git(&source, &["update-ref", "refs/remotes/origin/main", "HEAD"]);

    let version = crate::shots_program::VERSIONS.comparison.as_str();
    let covering = cache(&places, &waiting, "mac.", version);
    let before = cache(&places, &base, "mac.welcome.", version);
    let other_comparison = cache(&places, &waiting, "mac.welcome.", "0000");
    let other_capture = cache_by(
        &places,
        &waiting,
        "mac.welcome.",
        "0000",
        &crate::shots_program::VERSIONS.comparison,
    );
    let other_prefix = cache(&places, &waiting, "mac.seek.", version);
    let other_environment = {
        let key = Key {
            repo: BAE.to_string(),
            commit: waiting.clone(),
            lane: Lane::Mac,
            environment: "25G241-27B52".to_string(),
            prefix: "mac.".to_string(),
            versions: crate::shots_program::Versions {
                capture: crate::shots_program::VERSIONS.capture.clone(),
                comparison: version.to_string(),
            },
        };
        let staging = stage(&places, &key).unwrap();
        std::fs::create_dir_all(staging.report()).unwrap();
        staging.finish(0, "ok\n").unwrap().folder
    };
    let landed_run = cache(&places, &landed, "mac.welcome.", version);

    let store = make_store_named(&places, "demo", &[BAE]);
    let path = store.database();
    record(&path, &build("waiting", &waiting, "mac.welcome.")).unwrap();
    record(&path, &build("landed", &landed, "mac.welcome.")).unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();

    let named = named_in(&places, BAE, &source, &conn, &path).unwrap();
    let work = BTreeSet::from(["waiting".to_string()]);
    assert_eq!(named.get(&covering), Some(&work));
    assert_eq!(
        named.get(&before),
        Some(&work),
        "the base run it compared with"
    );
    assert_eq!(
        named.get(&other_comparison),
        Some(&work),
        "its pictures are what a comparison again reads"
    );
    assert_eq!(
        named.get(&other_environment),
        Some(&work),
        "a run made in another environment is the build's run too"
    );
    assert_eq!(named.get(&other_capture), None);
    assert_eq!(named.get(&other_prefix), None);
    assert_eq!(named.get(&landed_run), None);
}
