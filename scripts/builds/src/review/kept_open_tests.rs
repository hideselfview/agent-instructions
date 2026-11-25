/// The table of kept verdicts as it was before it kept each verdict's reason.
pub const KEPT_OPEN_WITHOUT_REASON: &str = "CREATE TABLE review_kept_open (
    call_id INTEGER NOT NULL REFERENCES review_call (id),
    finding_id INTEGER NOT NULL REFERENCES review_finding (id),
    PRIMARY KEY (call_id, finding_id)
) STRICT, WITHOUT ROWID;";

/// Writes a verdict that kept a finding open into the schema from before
/// builds kept reasons: the state migrate.rs's move of that table starts from.
pub fn plant_before_reason(db: &rusqlite::Connection) {
    crate::review::record::tests::plant_finding_before_head_sites(db);
    db.execute_batch("INSERT INTO review_kept_open (call_id, finding_id) VALUES (1, 1);")
        .unwrap();
}
