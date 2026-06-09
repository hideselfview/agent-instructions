#!/usr/bin/env python3
"""Run the per-rule Codex reviewer from a local checkout."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace

import rules_review_common


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RULES_ROOT = SCRIPT_DIR.parents[1]


@dataclass(frozen=True)
class RuleResult:
    slug: str
    returncode: int
    output: Path
    stderr: Path
    violations: list[dict]
    error: str | None


def run_command(command: list[str], *, cwd: Path | None = None, input_text: str | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        command,
        cwd=cwd,
        input=input_text,
        capture_output=True,
        text=True,
        check=False,
    )


def checked_stdout(command: list[str], *, cwd: Path | None = None) -> str:
    result = run_command(command, cwd=cwd)
    if result.returncode != 0:
        message = result.stderr.strip() or result.stdout.strip() or f"{command[0]} exited {result.returncode}"
        raise SystemExit(message)
    return result.stdout.strip()


def discover_slugs(rules_root: Path, consumer: Path, project: str, exclude: str) -> list[str]:
    raw = checked_stdout(
        [
            sys.executable,
            str(rules_root / ".github" / "scripts" / "discover_rules.py"),
            "--rules-root",
            str(rules_root),
            "--consumer",
            str(consumer),
            "--project",
            project,
            "--exclude",
            exclude,
        ]
    )
    return json.loads(raw)


def derive_repo(consumer: Path) -> str:
    return checked_stdout(["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"], cwd=consumer)


def derive_sha(repo: str, pr_number: str) -> str:
    return checked_stdout(["gh", "pr", "view", pr_number, "--repo", repo, "--json", "headRefOid", "--jq", ".headRefOid"])


def prepare_rule_workspace(
    *,
    rule_dir: Path,
    rules_root: Path,
    consumer: Path,
    project: str,
    slug: str,
    repo: str,
    pr_number: str,
    diff: Path,
    view: Path,
) -> Path:
    rule_dir.mkdir(parents=True, exist_ok=True)
    rules_review_common.select_rule(rules_root, project, slug, rule_dir / "THE_RULE.md")
    shutil.copyfile(diff, rule_dir / "PR_DIFF.patch")
    shutil.copyfile(view, rule_dir / "PR_VIEW.json")
    rules_review_common.write_schema(rule_dir / "RULES_REVIEW_SCHEMA.json")
    rules_review_common.write_prompt(repo, pr_number, rule_dir / "RULES_REVIEW_PROMPT.md")

    repo_link = rule_dir / "CONSUMER_REPO"
    try:
        repo_link.symlink_to(consumer, target_is_directory=True)
        repo_reference = "./CONSUMER_REPO"
    except OSError:
        repo_reference = str(consumer)

    with (rule_dir / "RULES_REVIEW_PROMPT.md").open("a", encoding="utf-8") as f:
        f.write(
            "\nFor repository-file context beyond the diff, read the consumer checkout at "
            f"{repo_reference}. Do not modify it.\n"
        )

    return rule_dir / "RULES_REVIEW_PROMPT.md"


def run_codex_for_rule(
    *,
    slug: str,
    rule_dir: Path,
    codex_bin: str,
    model: str | None,
    effort: str | None,
    sandbox: str,
    extra_codex_args: list[str],
) -> RuleResult:
    output = rule_dir / "RULES_REVIEW_OUTPUT.json"
    stderr = rule_dir / "codex.stderr.log"
    stdout = rule_dir / "codex.stdout.log"
    prompt = (rule_dir / "RULES_REVIEW_PROMPT.md").read_text(encoding="utf-8")

    command = [
        codex_bin,
        "exec",
        "--ephemeral",
        "--ignore-rules",
        "--config",
        "project_doc_max_bytes=0",
        "--output-schema",
        str(rule_dir / "RULES_REVIEW_SCHEMA.json"),
        "--sandbox",
        sandbox,
        "-C",
        str(rule_dir),
        "-o",
        str(output),
    ]
    if model:
        command.extend(["--model", model])
    if effort:
        command.extend(["--config", f'model_reasoning_effort="{effort}"'])
    command.extend(extra_codex_args)
    command.append("-")

    with stdout.open("w", encoding="utf-8") as stdout_file, stderr.open("w", encoding="utf-8") as stderr_file:
        try:
            result = subprocess.run(
                command,
                input=prompt,
                text=True,
                stdout=stdout_file,
                stderr=stderr_file,
                check=False,
            )
        except OSError as e:
            return RuleResult(
                slug=slug,
                returncode=127,
                output=output,
                stderr=stderr,
                violations=[],
                error=f"failed to start {codex_bin}: {e}",
            )

    findings, parse_error = rules_review_common.load_findings(output)
    violations = findings["violations"] if findings else []
    error = parse_error
    if result.returncode != 0:
        tail = "\n".join(stderr.read_text(encoding="utf-8").splitlines()[-8:])
        error = f"codex exited {result.returncode}" + (f": {tail}" if tail else "")

    return RuleResult(
        slug=slug,
        returncode=result.returncode,
        output=output,
        stderr=stderr,
        violations=violations,
        error=error,
    )


def summarize(result: RuleResult) -> str:
    if result.error:
        return f"ERROR {result.slug}: {result.error}"
    if not result.violations:
        return f"OK {result.slug}: 0"
    return f"FINDINGS {result.slug}: {len(result.violations)}"


def write_summary(path: Path, repo: str, pr_number: str, sha: str | None, results: list[RuleResult]) -> None:
    payload = {
        "repo": repo,
        "pr_number": pr_number,
        "sha": sha,
        "results": [
            {
                "slug": result.slug,
                "returncode": result.returncode,
                "output": str(result.output),
                "stderr": str(result.stderr),
                "error": result.error,
                "violations": result.violations,
            }
            for result in results
        ],
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def positive_int(raw: str) -> int:
    value = int(raw)
    if value < 1:
        raise argparse.ArgumentTypeError("must be >= 1")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rules-root", type=Path, default=DEFAULT_RULES_ROOT)
    parser.add_argument("--consumer", type=Path, default=Path.cwd())
    parser.add_argument("--project", required=True)
    parser.add_argument("--repo")
    parser.add_argument("--pr-number", required=True)
    parser.add_argument("--sha")
    parser.add_argument("--exclude", default="[]")
    parser.add_argument("--slug", action="append", default=[], help="review only this discovered rule slug; repeatable")
    parser.add_argument("--jobs", type=positive_int, default=1)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--model")
    parser.add_argument("--effort")
    parser.add_argument("--sandbox", choices=["read-only", "workspace-write"], default="read-only")
    parser.add_argument("--codex-arg", action="append", default=[], help="extra argument passed to codex exec; repeatable")
    parser.add_argument("--list", action="store_true", help="print discovered slugs and exit")
    parser.add_argument("--post", action="store_true", help="post findings as GitHub PR review comments")
    parser.add_argument("--fail-on-findings", action="store_true")
    args = parser.parse_args()

    rules_root = args.rules_root.resolve()
    consumer = args.consumer.resolve()
    repo = args.repo or derive_repo(consumer)
    slugs = discover_slugs(rules_root, consumer, args.project, args.exclude)

    if args.slug:
        requested = set(args.slug)
        discovered = set(slugs)
        unknown = sorted(requested - discovered)
        if unknown:
            raise SystemExit(f"requested slug(s) not discovered: {unknown}")
        slugs = [slug for slug in slugs if slug in requested]

    print(json.dumps(slugs) if args.list else f"discovered {len(slugs)} rule(s)")
    if args.list:
        return 0
    if not slugs:
        return 0

    sha = args.sha or (derive_sha(repo, args.pr_number) if args.post else None)
    work_dir = (args.work_dir or Path(tempfile.mkdtemp(prefix="local-codex-rules-review-"))).resolve()
    work_dir.mkdir(parents=True, exist_ok=True)
    print(f"work_dir={work_dir}", flush=True)

    diff = work_dir / "PR_DIFF.patch"
    view = work_dir / "PR_VIEW.json"
    try:
        rules_review_common.fetch_pr_context(repo, args.pr_number, diff, view)
    except subprocess.CalledProcessError as e:
        raise SystemExit(f"{e.cmd} exited {e.returncode} while fetching PR context") from e

    rule_dirs: dict[str, Path] = {}
    for slug in slugs:
        rule_dir = work_dir / "rules" / slug
        prepare_rule_workspace(
            rule_dir=rule_dir,
            rules_root=rules_root,
            consumer=consumer,
            project=args.project,
            slug=slug,
            repo=repo,
            pr_number=args.pr_number,
            diff=diff,
            view=view,
        )
        rule_dirs[slug] = rule_dir

    results_by_slug: dict[str, RuleResult] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        futures = {
            executor.submit(
                run_codex_for_rule,
                slug=slug,
                rule_dir=rule_dirs[slug],
                codex_bin=args.codex_bin,
                model=args.model,
                effort=args.effort,
                sandbox=args.sandbox,
                extra_codex_args=args.codex_arg,
            ): slug
            for slug in slugs
        }
        for future in concurrent.futures.as_completed(futures):
            result = future.result()
            results_by_slug[result.slug] = result
            print(summarize(result), flush=True)

    results = [results_by_slug[slug] for slug in slugs]
    write_summary(work_dir / "SUMMARY.json", repo, args.pr_number, sha, results)

    if args.post:
        if not sha:
            raise SystemExit("--post needs --sha or a derivable PR head SHA")
        for result in results:
            if result.error:
                continue
            rules_review_common.post_findings(
                SimpleNamespace(
                    findings=str(result.output),
                    repo=repo,
                    pr_number=args.pr_number,
                    sha=sha,
                    slug=result.slug,
                )
            )

    error_count = sum(1 for result in results if result.error)
    finding_count = sum(len(result.violations) for result in results)
    print(f"summary: errors={error_count} findings={finding_count} summary={work_dir / 'SUMMARY.json'}")

    if error_count:
        return 2
    if args.fail_on_findings and finding_count:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
