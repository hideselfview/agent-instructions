use super::*;
use sha2::{Digest, Sha256};

#[test]
fn each_copy_hashes_to_its_sha256_and_is_found_by_it() {
    for entry in COPIES {
        let actual: String = Sha256::digest(entry.text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, entry.sha256, "{}", copy_file(entry.sha256));
        assert_eq!(copy(entry.sha256), Some(entry));
    }
    assert_eq!(copy("0"), None);
}

#[test]
fn the_newest_copy_is_the_one_read_last() {
    let last = COPIES.iter().map(|c| c.read_at).max().unwrap();
    assert_eq!(newest().read_at, last);
}
