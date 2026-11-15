//! What `builds review` prints: the findings by group, then one line of totals.

use crate::review::record::{Call, Finding};

/// A run's calls with their findings, by group.
pub fn run_report(calls: &[Call], findings: &[Finding]) -> String {
    let mut out = String::new();
    for call in calls {
        let own: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.group == call.group && f.run_id == call.run_id)
            .collect();
        let accepted: Vec<&&Finding> = own.iter().filter(|f| f.counts()).collect();
        if let Some(why) = &call.failure {
            out.push_str(&format!(
                "{} ({}): the review failed: {why}\n",
                call.group, call.model
            ));
            continue;
        }
        if accepted.is_empty() {
            out.push_str(&format!("{} ({}): no findings\n", call.group, call.model));
        } else {
            out.push_str(&format!(
                "{} ({}): {}\n",
                call.group,
                call.model,
                plural(accepted.len(), "finding")
            ));
        }
        for finding in accepted {
            out.push_str(&finding_text(finding));
        }
        for finding in own.iter().filter(|f| !f.counts()) {
            out.push_str(&format!(
                "  dropped by builds, finding {} ({}): {}\n",
                finding.id,
                finding.rule,
                finding.check_failed.as_deref().unwrap_or("")
            ));
        }
    }
    out.push_str(&totals(calls, findings));
    out.push('\n');
    out
}

/// A finding, indented under its group.
pub fn finding_text(finding: &Finding) -> String {
    let kind = if finding.blocking {
        "blocking"
    } else {
        "reported only"
    };
    let mut out = format!(
        "  finding {} ({kind}) {}, {}:{}\n    clause: {}\n    code: {}\n    problem: {}\n",
        finding.id,
        finding.rule,
        finding.file,
        finding.line,
        one_line(&finding.clause),
        one_line(&finding.code),
        one_line(&finding.problem),
    );
    if let Some((how, why)) = &finding.settled {
        out.push_str(&format!("    settled {how}: {}\n", one_line(why)));
    }
    if let Some(fix) = &finding.fix {
        out.push_str(&format!("    fixed by {}\n", fix.sha));
    }
    out
}

/// `3 groups, 2 findings (1 blocking), 1 dropped by builds, 1200 tokens in, 80 out`.
pub fn totals(calls: &[Call], findings: &[Finding]) -> String {
    let accepted = findings.iter().filter(|f| f.counts()).count();
    let blocking = findings.iter().filter(|f| f.counts() && f.blocking).count();
    let dropped = findings.len() - accepted;
    let tokens_in: i64 = calls.iter().map(|c| c.tokens_in).sum();
    let tokens_out: i64 = calls.iter().map(|c| c.tokens_out).sum();
    let cached: i64 = calls.iter().map(|c| c.tokens_cached).sum();
    let written: i64 = calls.iter().map(|c| c.tokens_written).sum();
    format!(
        "{}, {} ({blocking} blocking), {dropped} dropped by builds, {tokens_in} tokens in ({} uncached, {cached} read from the cache, {written} written to it), {tokens_out} out",
        plural(calls.len(), "group"),
        plural(accepted, "finding"),
        tokens_in - cached - written,
    )
}

pub fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
