use super::*;
use crate::review::rules;

/// The checkout this file is in: builds sits in scripts/builds.
fn checkout() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the checkout resolves")
}

fn rule(slug: &str, paths: &str, dir: &str) -> Rule {
    let mut rule = Rule::parse(slug, &format!("---\npaths:\n  - '{paths}'\n---\nbody\n"));
    rule.path = format!("rules/{dir}/{slug}.md");
    rule.group_dir = Some(dir.to_string());
    rule
}

fn group(dir: &str, extra: &str, rules: &[&str]) -> Group {
    Group::parse(
        dir,
        &format!("name = \"{dir}\"\nquestion = \"q\"\n{extra}"),
        rules.iter().map(|s| s.to_string()).collect(),
    )
    .unwrap()
}

fn groups(groups: Vec<Group>) -> Groups {
    Groups { groups }
}

#[test]
fn every_rule_of_the_checkout_is_in_a_group_folder() {
    let root = checkout();
    let groups = Groups::load(&root).unwrap();
    let rules = rules::load(&root, None).unwrap();
    assert!(!rules.is_empty());
    assert_eq!(groups.faults(&rules), Vec::<String>::new());
}

#[test]
fn a_group_runs_opus_unless_it_names_a_model() {
    assert_eq!(group("a", "", &["x"]).model, "opus");
    assert_eq!(group("a", "model = \"sonnet\"\n", &["x"]).model, "sonnet");
}

#[test]
fn faults_name_a_rule_outside_a_group_folder_an_empty_group_and_a_bad_model() {
    let g = groups(vec![
        group("a", "", &["one"]),
        group("b", "model = \"gpt\"\n", &[]),
    ]);
    let mut loose = rule("loose", "**", "a");
    loose.group_dir = None;
    let rules = vec![
        rule("one", "**", "a"),
        rule("three", "**", "nogroup"),
        loose,
    ];
    let faults = g.faults(&rules);
    assert!(
        faults
            .iter()
            .any(|f| f.contains("three (rules/nogroup/three.md) is in no group")),
        "{faults:?}"
    );
    assert!(faults.iter().any(|f| f.contains("loose")));
    assert!(faults.iter().any(|f| f.contains("group b has no rule")));
    assert!(faults.iter().any(|f| f.contains("names model gpt")));
}

#[test]
fn a_group_runs_when_one_of_its_rules_covers_a_changed_path() {
    let g = groups(vec![
        group("swift", "", &["s1", "s2", "not-in-this-project"]),
        group("rust", "", &["r1"]),
    ]);
    let rules = vec![
        rule("s1", "**/*.swift", "swift"),
        rule("s2", "**/*.kt", "swift"),
        rule("r1", "**/*.rs", "rust"),
    ];
    let picked = g.pick(
        &rules,
        &["bae-macos/A.swift".to_string()],
        &Globs::default(),
    );
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].0.name, "swift");
    let slugs: Vec<&str> = picked[0].1.iter().map(|r| r.slug.as_str()).collect();
    assert_eq!(slugs, ["s1", "s2"]);
    assert!(
        g.pick(&rules, &["README.md".to_string()], &Globs::default())
            .is_empty()
    );
}

#[test]
fn a_rule_marked_review_false_never_runs_its_group() {
    let g = groups(vec![group("g", "", &["x"])]);
    let rules = vec![Rule::parse(
        "x",
        "---\npaths: ['**']\nreview: false\n---\nb\n",
    )];
    assert!(
        g.pick(&rules, &["a".to_string()], &Globs::default())
            .is_empty()
    );
}

#[test]
fn a_group_that_lists_sites_says_what_a_site_is() {
    let g = groups(vec![group("g", "enumerate = true\n", &["a", "b"])]);
    let rules = vec![rule("a", "**", "g"), rule("b", "**", "g")];
    assert!(
        g.faults(&rules)
            .iter()
            .any(|f| f.contains("lists sites, so it says what a site is"))
    );
}

#[test]
fn groups_load_from_their_folders_with_the_rules_of_both_trees() {
    let scratch = crate::testing::Scratch::new("groups-load");
    let root = scratch.dir.join("ai");
    for dir in ["rules/g", "projects/p-rules/g", "projects/p-rules/other"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(
        root.join("rules/g/group.toml"),
        "name = \"G\"\nquestion = \"q\"\n",
    )
    .unwrap();
    std::fs::write(root.join("rules/g/a.md"), "a\n").unwrap();
    std::fs::write(root.join("projects/p-rules/g/b.md"), "b\n").unwrap();
    std::fs::write(root.join("projects/p-rules/other/c.md"), "c\n").unwrap();
    let loaded = Groups::load(&root).unwrap();
    assert_eq!(loaded.groups.len(), 1);
    assert_eq!(loaded.groups[0].rules, ["a", "b"]);
    let every = rules::load(&root, None).unwrap();
    let faults = loaded.faults(&every);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert!(faults[0].contains("c (projects/p-rules/other/c.md) is in no group"));
    std::fs::write(root.join("projects/p-rules/g/a.md"), "dup\n").unwrap();
    assert!(rules::load(&root, None).is_err());
}

/// The rules about wording are in a copy group, so a review of any change
/// asks none of them, and the copy review asks them. Localization judges
/// whether the catalogs hold every string, so a review asks it.
#[test]
fn a_review_of_the_checkout_runs_no_wording_rule() {
    let root = checkout();
    let groups = Groups::load(&root).unwrap();
    let rules = rules::load(&root, Some("bae")).unwrap();
    let wording = [
        "write-plain-literal-active-sentences",
        "ui-text-says-each-thing-once-in-gentle-plain-words",
        "no-dash-as-sentence-punctuation",
        "spell-bae-and-disc-id-one-way",
    ];
    let changed = vec![
        "bae-macos/bae/Views/Welcome.swift".to_string(),
        "spec/bae.md".to_string(),
        "bae-android/app/src/main/res/values/strings.xml".to_string(),
    ];
    let asked: Vec<&str> = groups
        .pick(&rules, &changed, &Globs::default())
        .into_iter()
        .flat_map(|(_, members)| members.into_iter().map(|rule| rule.slug.as_str()))
        .collect();
    assert!(
        asked.contains(&"localization-stays-in-sync-with-ui"),
        "{asked:?}"
    );
    for slug in wording {
        assert!(!asked.contains(&slug), "{slug}");
    }
    let copy: Vec<&str> = groups
        .pick_copy(&rules, &changed, &Globs::default())
        .into_iter()
        .flat_map(|(_, members)| members.into_iter().map(|rule| rule.slug.as_str()))
        .collect();
    for slug in wording {
        assert!(copy.contains(&slug), "{slug}: {copy:?}");
    }
}

/// A change to bae's boards and design system asks only the rules that judge
/// design files (decision #364): how a menu sizes and how a preview lays out
/// its rows, never how a shape changes or what code is needed.
#[test]
fn a_review_of_bae_design_files_asks_only_the_rules_that_judge_design_files() {
    let root = checkout();
    let groups = Groups::load(&root).unwrap();
    let rules = rules::load(&root, Some("bae")).unwrap();
    let design = Globs {
        places: vec![
            "design/mocks/*.html".into(),
            "design/mocks/canvas.json".into(),
            "design/system/components/*/".into(),
        ],
        sources: vec![
            "design/system/components/src/*".into(),
            "design/system/tokens.json".into(),
        ],
        spec: vec!["spec/bae.md".into()],
    };
    let changed = vec![
        "design/mocks/mac.album.dc.html".to_string(),
        "design/mocks/canvas.json".to_string(),
        "design/system/components/MacRow/preview.html".to_string(),
        "design/system/components/src/10-mac.js".to_string(),
    ];
    let asked: Vec<&Rule> = groups
        .pick(&rules, &changed, &design)
        .into_iter()
        .flat_map(|(_, members)| members)
        .collect();
    let slugs: Vec<&str> = asked.iter().map(|rule| rule.slug.as_str()).collect();
    for slug in [
        "menus-size-to-their-items",
        "component-previews-hold-their-content-still",
    ] {
        assert!(slugs.contains(&slug), "{slug}: {slugs:?}");
    }
    for slug in [
        "change-a-shape-in-one-commit-with-no-compatibility-code",
        "write-only-what-is-needed-today",
        "every-change-agrees-with-its-product-spec",
    ] {
        assert!(!slugs.contains(&slug), "{slug}: {slugs:?}");
    }
    assert!(asked.iter().all(|rule| rule.design), "{slugs:?}");
}
