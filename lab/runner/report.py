"""report.py - writes the leak lab report (/out/report.md) and decides the runner's exit code:
non-zero if any check found a leak or any scenario could not run (a broken lab must not pass)."""
import datetime


def write(path, scenarios, findings, notes):
    """Writes the markdown report. `scenarios` is [(name, ok, detail)], `findings` a list of
    analyzer.Finding, `notes` a list of free-text lines (observations that are not pass/fail).
    Returns the exit code: 0 only when every scenario ran and nothing was found."""
    lines = [f"# Nyx leak lab report", "", f"Run: {datetime.datetime.now(datetime.UTC):%Y-%m-%d %H:%M:%S} UTC", "",
             "## Scenarios", "", "| Scenario | Ran | Detail |", "|---|---|---|"]
    lines += [f"| {name} | {'yes' if ok else 'NO'} | {detail} |" for name, ok, detail in scenarios]
    lines += ["", f"## Findings ({len(findings)})", ""]
    if findings:
        lines += ["| # | Check | Scenario | Detail |", "|---|---|---|---|"]
        lines += [f"| {i} | {f.check} | {f.scenario} | {f.detail.replace('|', '/')} |" for i, f in enumerate(findings, 1)]
    else:
        lines.append("None.")
    lines += ["", "## Notes", ""] + [f"- {n}" for n in notes]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    return 0 if not findings and all(ok for _, ok, _ in scenarios) else 1
