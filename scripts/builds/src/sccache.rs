//! What the shared sccache server says about itself. Only a server that is
//! already listening is asked: asking with none running starts one.

use std::process::{Command, Stdio};

/// Is a server listening on sccache's port?
pub fn listening() -> bool {
    Command::new("lsof")
        .args(["-nP", "-iTCP:4226", "-sTCP:LISTEN"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The lines `sccache --show-stats` prints; empty when it can't be asked.
pub fn stats() -> Vec<String> {
    let Ok(output) = Command::new("sccache")
        .arg("--show-stats")
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The count after <label> in <stats> ("Compile requests executed  152034").
pub fn count(stats: &[String], label: &str) -> Option<u64> {
    stats.iter().find_map(|line| {
        let rest = line.strip_prefix(label)?;
        if !rest.starts_with(' ') {
            return None;
        }
        rest.trim().parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        let stats = vec![
            "Compile requests                  209331".to_string(),
            "Compile requests executed         153775".to_string(),
        ];
        assert_eq!(count(&stats, "Compile requests executed"), Some(153775));
        assert_eq!(count(&stats, "Compile requests"), Some(209331));
        assert_eq!(count(&stats, "Cache hits"), None);
    }
}
