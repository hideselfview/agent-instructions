use super::*;
use crate::lists::fixtures::{Refusals, row, v};

#[test]
fn every_named_constraint_of_the_projects_tables_refuses_its_row() {
    let db = crate::db::memory();
    let project = [("singleton", v(1)), ("name", v("demo")), ("canvas", v(0))];
    plant(&db, "project", &project).unwrap();
    let mut refused = Refusals::new(include_str!("project.sql"));
    refused.check(
        "project_is_single",
        plant(&db, "project", &row(&project, &[("singleton", v(2))])),
    );
    refused.check(
        "project_canvas_is_yes_or_no",
        plant(&db, "project", &row(&project, &[("canvas", v(2))])),
    );
    for name in ["", ".hidden", "a/b", "a b", "é"] {
        refused.check(
            "project_name_is_a_folder_name",
            plant(&db, "project", &row(&project, &[("name", v(name))])),
        );
    }
    for name in ["", "a/b"] {
        refused.check(
            "repo_name_is_a_directory_name",
            plant(&db, "repo", &[("name", v(name))]),
        );
    }
    plant(&db, "repo", &[("name", v("bae"))]).unwrap();
    let glob = [
        ("repo", v("bae")),
        ("kind", v("place")),
        ("glob", v("design/*")),
    ];
    plant(&db, "design_glob", &glob).unwrap();
    refused.check(
        "design_glob_kind_known",
        plant(&db, "design_glob", &row(&glob, &[("kind", v("other"))])),
    );
    refused.check(
        "design_glob_says_something",
        plant(&db, "design_glob", &row(&glob, &[("glob", v(" "))])),
    );
    refused.done();
}

#[test]
fn a_glob_is_a_set_and_names_a_repo_of_the_project() {
    let db = crate::db::memory();
    init(
        &db,
        &NewProject {
            name: "demo".into(),
            canvas: false,
            repos: vec!["bae".into()],
            places: parse_globs("design-places", "bae:a,bae:a,bae:b").unwrap(),
            sources: Globs::new(),
            specs: Globs::new(),
        },
    )
    .unwrap();
    assert_eq!(read(&db).unwrap().places["bae"], ["a", "b"]);
    let unknown = plant(
        &db,
        "design_glob",
        &[("repo", v("nope")), ("kind", v("place")), ("glob", v("x"))],
    );
    assert!(unknown.unwrap_err().to_string().contains("FOREIGN KEY"));
}

#[test]
fn a_project_with_a_glob_builds_cannot_match_is_refused() {
    let globs: Globs = [("bae".to_string(), vec!["design/**/".to_string()])]
        .into_iter()
        .collect();
    let new = NewProject {
        name: "bae".to_string(),
        canvas: false,
        repos: vec!["bae".to_string()],
        places: globs,
        sources: Globs::new(),
        specs: Globs::new(),
    };
    let refusal = new.refusal().unwrap();
    assert!(refusal.contains("design/**/"), "{refusal}");
}
