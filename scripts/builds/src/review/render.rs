//! What `builds review` prints: the findings by group, then one line of totals.

use crate::review::record::{Call, Finding, Settle, Settled};
use crate::text::{one_line, plural};

/// A run's calls with their findings, by group.
pub fn run_report(calls: &[Call], findings: &[Finding]) -> String {
    let mut out = String::new();
    for call in calls {
        let own: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.group == call.group && f.run_id == call.run_id)
            .collect();
        let accepted: Vec<&&Finding> = own.iter().filter(|f| f.counts()).collect();
        out.push_str(&call_line(
            &call.group,
            &call.model,
            call.failure.as_deref(),
            accepted.len(),
        ));
        if call.failure.is_some() {
            continue;
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

/// A call's line above its findings: its group and model, then why it
/// failed or how many findings it raised.
fn call_line(group: &str, model: &str, failure: Option<&str>, findings: usize) -> String {
    match (failure, findings) {
        (Some(why), _) => format!("{group} ({model}): the review failed: {why}\n"),
        (None, 0) => format!("{group} ({model}): no findings\n"),
        (None, count) => format!("{group} ({model}): {}\n", plural(count, "finding")),
    }
}

/// A finding, indented under its group.
pub fn finding_text(finding: &Finding) -> String {
    let mut out = format!(
        "  finding {} {}",
        finding.id,
        raised(
            finding.blocking,
            &finding.rule,
            &finding.file,
            finding.line,
            &finding.clause,
            &finding.code,
            &finding.problem
        )
    );
    match &finding.settled {
        None => {}
        Some(Settled::Once(settle)) => out.push_str(&settle_line("settled", settle)),
        Some(Settled::Again { first, second }) => {
            out.push_str(&settle_line("settled", first));
            out.push_str(&settle_line("settled again as", second));
        }
        Some(Settled::Allowed { confirmed, allowed }) => {
            out.push_str(&settle_line("settled", confirmed));
            out.push_str(&format!(
                "    the person allowed it to land, and {} recorded their words: {}\n",
                allowed.by,
                one_line(&allowed.why)
            ));
        }
    }
    if let Some(run) = finding.closed_by {
        out.push_str(&format!(
            "    closed by run {run}: the code at its place no longer has the problem\n"
        ));
    }
    out
}

/// What a reviewer raised, after the finding's number: whether it blocks,
/// its rule and place, then its clause, code and problem.
fn raised(
    blocking: bool,
    rule: &str,
    file: &str,
    line: i64,
    clause: &str,
    code: &str,
    problem: &str,
) -> String {
    let kind = if blocking {
        "blocking"
    } else {
        "reported only"
    };
    format!(
        "({kind}) {rule}, {file}:{line}\n    clause: {}\n    code: {}\n    problem: {}\n",
        one_line(clause),
        one_line(code),
        one_line(problem),
    )
}

/// A settle as one indented line: `<word> <how> by <who>: <why>`.
fn settle_line(word: &str, settle: &Settle) -> String {
    format!(
        "    {word} {} by {}: {}\n",
        settle.how,
        settle.by,
        one_line(&settle.why)
    )
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
