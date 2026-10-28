"""report.py - writes the leak lab report (/out/report.md) and decides the runner's exit code:
non-zero if any check found a real leak, any scenario could not run, or the positive control's
deliberate leaks were not all detected (a broken or blind lab must not pass)."""
import datetime

from analyzer import CONTROL_SIGNATURES


def lab_only(value, valid):
    """Parses LAB_ONLY (comma-separated scenario names; empty = every scenario) into a set. An
    unknown name raises ValueError listing the valid ones, so a typo fails the run up front
    instead of silently skipping every scenario and reporting a clean lab."""
    names = {n.strip() for n in value.split(",") if n.strip()}
    unknown = sorted(names - set(valid))
    if unknown:
        raise ValueError(f"LAB_ONLY: unknown scenario {', '.join(unknown)}; valid: {', '.join(valid)}")
    return names


def write(path, scenarios, findings, notes, control_missing, partial=None):
    """Writes the markdown report. `scenarios` is [(name, ok, detail)], `findings` the REAL
    analyzer.Finding list (positive-control leaks already set aside), `notes` free-text lines,
    `control_missing` the names of control leaks that were not detected, `partial` the LAB_ONLY
    value of a subset run (None for a full run): its verdict says PARTIAL RUN, so a subset run
    never reads as a clean lab. Returns the exit code: 0 only when every scenario that ran, ran,
    nothing real was found and the control saw everything."""
    failed = sum(1 for _, ok, _ in scenarios if not ok)
    verdict = (f"{failed} scenario{'' if failed == 1 else 's'} failed (infrastructure), "
               f"{len(findings)} finding{'' if len(findings) == 1 else 's'} (leaks), "
               f"positive control {'complete' if not control_missing else 'BLIND'}")
    if partial:
        verdict = f"PARTIAL RUN (LAB_ONLY={partial}), not a full lab: {verdict}"
    lines = [f"# Nyx leak lab report", "", f"Run: {datetime.datetime.now(datetime.UTC):%Y-%m-%d %H:%M:%S} UTC", "",
             f"Result: {verdict}", "",
             "## Scenarios", "", "Ran = NO is an infrastructure failure: the scenario could not do its job (not a leak).", "",
             "| Scenario | Ran | Detail |", "|---|---|---|"]
    lines += [f"| {name} | {'yes' if ok else 'NO'} | {detail.replace('|', '/')} |" for name, ok, detail in scenarios]
    lines += ["", f"## Findings: leaks ({len(findings)})", ""]
    if findings:
        lines += ["| # | Check | Scenario | Detail |", "|---|---|---|---|"]
        lines += [f"| {i} | {f.check} | {f.scenario} | {f.detail.replace('|', '/')} |" for i, f in enumerate(findings, 1)]
    else:
        lines.append("None.")
    lines += ["", "## Positive control: " + ("all detected" if not control_missing else "LAB BLIND"), "",
              "Deliberate leaks from client-a's namespace, excluded from the findings above:", ""]
    lines += [f"- {sig[0]}: {'NOT DETECTED (lab blind)' if sig[0] in control_missing else 'detected'}" for sig in CONTROL_SIGNATURES]
    lines += ["", "## Notes", ""] + [f"- {n}" for n in notes]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    return 0 if not findings and not control_missing and all(ok for _, ok, _ in scenarios) else 1
