use super::*;

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(str::to_string).collect()
}

#[test]
fn draws_takes_the_repos_picture_folder_unless_it_names_one() {
    let store = Some(PathBuf::from("/Volumes/Builds/shots/bae/pictures"));
    assert_eq!(
        with_board_store(&words("draws --at a --against b"), store.clone()),
        words("draws --at a --against b --pictures /Volumes/Builds/shots/bae/pictures")
    );
    let named = words("draws --at a --against b --pictures /x");
    assert_eq!(with_board_store(&named, store.clone()), named);
    let named = words("draws --at a --against b --pictures=/x");
    assert_eq!(with_board_store(&named, store), named);
}

#[test]
fn only_draws_gets_a_picture_folder_and_only_in_a_checkout() {
    let store = Some(PathBuf::from("/s"));
    let run = words("run --select mac.");
    assert_eq!(with_board_store(&run, store), run);
    let draws = words("draws --at a --against b");
    assert_eq!(with_board_store(&draws, None), draws);
}
