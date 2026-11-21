use super::*;
use crate::testing::Scratch;

/// An asset of group <group> named <name>, whose bytes have the sha256
/// <sha256>.
fn asset(group: &str, name: &str, sha256: &str) -> Asset {
    Asset {
        path: format!("project/assets/{group}/{name}"),
        group: group.to_string(),
        name: name.to_string(),
        size: 3,
        media_type: "image/svg+xml".to_string(),
        sha256: sha256.to_string(),
    }
}

fn index(value: Value) -> Map<String, Value> {
    let Value::Object(index) = value else {
        panic!("an index is an object")
    };
    index
}

/// Pictures go as uploads, with their sizes, media types and sha256s; text
/// files go as files; a file of a kind the artifact type keeps neither way,
/// or one in no group, is refused.
#[test]
fn assets_are_uploads_or_text_files_by_their_kind() {
    let scratch = Scratch::new("ds-assets");
    let assets = scratch.dir.join(SYSTEM).join("assets");
    std::fs::create_dir_all(assets.join("Logos/parts")).unwrap();
    std::fs::write(assets.join("Logos/README.md"), "Logos.\n").unwrap();
    std::fs::write(assets.join("Logos/mark.svg"), "abc").unwrap();
    std::fs::write(assets.join("Logos/parts/ear.PNG"), "png").unwrap();
    let (texts, uploads) = read(&scratch.dir).unwrap();
    assert_eq!(
        texts.into_iter().collect::<Vec<_>>(),
        [(
            "project/assets/Logos/README.md".to_string(),
            "Logos.\n".to_string()
        )]
    );
    let found: Vec<&Asset> = uploads.iter().map(|(asset, _)| asset).collect();
    assert_eq!(
        found,
        [
            &Asset {
                media_type: "image/svg+xml".to_string(),
                ..asset(
                    "Logos",
                    "mark.svg",
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                )
            },
            &Asset {
                media_type: "image/png".to_string(),
                ..asset(
                    "Logos",
                    "parts/ear.PNG",
                    &crate::text::hex(&Sha256::digest(b"png"))
                )
            },
        ]
    );
    assert_eq!(uploads[0].1, b"abc");

    std::fs::write(assets.join("Logos/font.woff2"), "x").unwrap();
    let refused = read(&scratch.dir).unwrap_err().text();
    assert!(refused.contains("Logos/font.woff2"), "{refused}");
    std::fs::remove_file(assets.join("Logos/font.woff2")).unwrap();
    std::fs::write(assets.join("loose.svg"), "x").unwrap();
    let refused = read(&scratch.dir).unwrap_err().text();
    assert!(refused.contains("is in no group"), "{refused}");
}

/// A repo with no assets folder publishes no asset.
#[test]
fn a_design_system_without_assets_has_none() {
    let scratch = Scratch::new("ds-no-assets");
    std::fs::create_dir_all(scratch.dir.join(SYSTEM)).unwrap();
    let (texts, uploads) = read(&scratch.dir).unwrap();
    assert!(texts.is_empty() && uploads.is_empty());
}

/// An asset needs an upload unless the index records an upload of its very
/// bytes.
#[test]
fn only_assets_whose_bytes_the_index_lacks_are_uploaded() {
    let live = index(json!({
        "assetGroups": {"Logos": {"files": {
            "same.svg": {"name": "same.svg", "blob": "b1", "sha256": "s1"},
            "changed.svg": {"name": "changed.svg", "blob": "b2", "sha256": "old"},
            "unsure.svg": {"name": "unsure.svg", "blob": "b3"},
            "my~20mark.svg": {"name": "my mark.svg", "blob": "b4", "sha256": "s4"}
        }}}
    }));
    let all = [
        asset("Logos", "same.svg", "s1"),
        asset("Logos", "changed.svg", "s2"),
        asset("Logos", "unsure.svg", "s3"),
        asset("Logos", "my mark.svg", "s4"),
        asset("Logos", "new.svg", "s5"),
        asset("Icons", "same.svg", "s1"),
    ];
    let names: Vec<&str> = to_upload(&live, &all)
        .unwrap()
        .into_iter()
        .map(|asset| asset.path.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "project/assets/Logos/changed.svg",
            "project/assets/Logos/unsure.svg",
            "project/assets/Logos/new.svg",
            "project/assets/Icons/same.svg",
        ]
    );
}

/// The new records name the new uploads and keep the uploads of unchanged
/// bytes; the groups and their files keep the index's order with new ones
/// after; a group's other keys stay; the uploads nothing names any more are
/// returned for deletion.
#[test]
fn the_records_name_every_asset_at_the_commit_and_free_the_rest() {
    let mut live = index(json!({
        "title": "demo",
        "groups": ["Old", "Logos"],
        "assetGroups": {
            "Logos": {"name": "Logos", "tile": "l", "order": ["b.svg", "a.svg", "gone.svg"], "files": {
                "a.svg": {"name": "a.svg", "blob": "keep", "sha256": "sa"},
                "b.svg": {"name": "b.svg", "blob": "stale", "sha256": "old"},
                "gone.svg": {"name": "gone.svg", "blob": "gone", "sha256": "sg"}
            }},
            "Old": {"name": "Old", "files": {"x.svg": {"name": "x.svg", "blob": "old-group"}}}
        }
    }));
    let all = [
        asset("Logos", "a.svg", "sa"),
        asset("Logos", "b.svg", "sb"),
        asset("Logos", "c.svg", "sc"),
        asset("Icons", "i.svg", "si"),
    ];
    let new: BTreeMap<String, String> = [
        ("project/assets/Logos/b.svg", "new-b"),
        ("project/assets/Logos/c.svg", "new-c"),
        ("project/assets/Icons/i.svg", "new-i"),
    ]
    .into_iter()
    .map(|(path, id)| (path.to_string(), id.to_string()))
    .collect();
    let unused = set_records(&mut live, &all, &new).unwrap();
    assert_eq!(unused, ["gone", "old-group", "stale"]);
    assert_eq!(live["title"], json!("demo"));
    assert_eq!(live["groups"], json!(["Logos", "Icons"]));
    let logos = &live["assetGroups"]["Logos"];
    assert_eq!(logos["tile"], json!("l"));
    assert_eq!(logos["order"], json!(["b.svg", "a.svg", "c.svg"]));
    assert_eq!(logos["files"]["a.svg"]["blob"], json!("keep"));
    assert_eq!(
        logos["files"]["b.svg"],
        json!({"name": "b.svg", "blob": "new-b", "size": 3, "type": "image/svg+xml", "sha256": "sb"})
    );
    assert_eq!(live["assetGroups"]["Icons"]["name"], json!("Icons"));
    assert_eq!(live["assetGroups"]["Icons"]["order"], json!(["i.svg"]));
    assert!(live["assetGroups"].get("Old").is_none());
}

/// An asset that neither got an upload nor has a record of its bytes fails
/// the index, and the session writes the files again.
#[test]
fn an_asset_with_no_upload_of_its_bytes_is_refused() {
    let mut live = index(json!({"assetGroups": {}}));
    let refused = set_records(
        &mut live,
        &[asset("Logos", "a.svg", "sa")],
        &BTreeMap::new(),
    )
    .unwrap_err()
    .text();
    assert!(
        refused.contains("project/assets/Logos/a.svg has no upload"),
        "{refused}"
    );
}

#[test]
fn an_upload_id_comes_from_its_url_or_is_the_id() {
    assert_eq!(upload_id("/_blob/abc123").unwrap(), "abc123");
    assert_eq!(
        upload_id("https://claude.ai/artifact/X/_blob/abc123").unwrap(),
        "abc123"
    );
    assert_eq!(upload_id("abc123").unwrap(), "abc123");
    assert!(upload_id("/_blob/").is_err());
    assert!(upload_id("a b").is_err());
}

/// Each part of the index's asset records that is not the shape the artifact
/// type writes fails, naming it, so builds never writes over what it cannot
/// read; a part the index leaves out is empty.
#[test]
fn asset_records_of_the_wrong_shape_are_refused_naming_them() {
    let all = [asset("Logos", "a.svg", "sa")];
    let new: BTreeMap<String, String> =
        [("project/assets/Logos/a.svg".to_string(), "id".to_string())].into();
    let cases = [
        (json!({"assetGroups": []}), "assetGroups is []"),
        (json!({"groups": "Logos"}), "groups is \"Logos\""),
        (json!({"groups": ["Logos", 2]}), "groups holds 2"),
        (
            json!({"assetGroups": {"Logos": 1}}),
            "assetGroups.Logos is 1",
        ),
        (
            json!({"assetGroups": {"Logos": {"order": {}}}}),
            "assetGroups.Logos.order is {}",
        ),
        (
            json!({"assetGroups": {"Logos": {"files": []}}}),
            "assetGroups.Logos.files is []",
        ),
        (
            json!({"assetGroups": {"Logos": {"files": {"a.svg": {"name": "a.svg"}}}}}),
            "assetGroups.Logos.files.a.svg names the upload null",
        ),
        (
            json!({"assetGroups": {"Logos": {"files": {"a.svg": {"blob": "b", "sha256": 1}}}}}),
            "assetGroups.Logos.files.a.svg gives the sha256 1",
        ),
    ];
    for (live, expected) in cases {
        let mut live = index(live);
        let refused = to_upload(&live, &all).unwrap_err().text();
        assert!(refused.contains(expected), "{refused}");
        let refused = set_records(&mut live, &all, &new).unwrap_err().text();
        assert!(refused.contains(expected), "{refused}");
    }
    let mut empty = index(json!({}));
    assert_eq!(to_upload(&empty, &all).unwrap().len(), 1);
    assert!(set_records(&mut empty, &all, &new).unwrap().is_empty());
    assert_eq!(empty["groups"], json!(["Logos"]));
}

/// A name the index's order gives twice is kept once, where it first stands.
#[test]
fn an_order_keeps_each_name_once() {
    let kept: Vec<String> = ["b", "a", "b"].map(String::from).to_vec();
    let wanted = ["a", "b", "c"].map(String::from).into_iter();
    assert_eq!(kept_then_new(&kept, wanted), ["b", "a", "c"]);
}
