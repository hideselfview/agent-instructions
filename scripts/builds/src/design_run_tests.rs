use super::*;

#[test]
fn a_record_an_amend_and_a_board_comparison_are_design_commands() {
    let of = |line: &str| {
        let argv: Vec<String> = line.split(' ').map(str::to_string).collect();
        DesignCommand::of(&argv)
    };
    let record = Some(DesignCommand::Record);
    assert_eq!(of("builds design record --request r.json"), record);
    assert_eq!(
        of("builds --agent a1 design amend 4 --request r.json"),
        record
    );
    assert_eq!(
        of("builds shots draws --at HEAD --against origin/main"),
        Some(DesignCommand::Comparison)
    );
    assert_eq!(of("builds design show 4"), None);
    assert_eq!(of("builds shots findings demo HEAD"), None);
}
