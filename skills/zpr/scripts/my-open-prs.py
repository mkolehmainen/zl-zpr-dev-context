#!/usr/bin/env python3
"""Report review state of every open PR authored by a user in org-zpr.

Designed as an output-hash *monitor* script: output must be STABLE (sorted, no
timestamps, no clock values) so hash-based change detection only fires when
something actually changed on a PR.

For each open PR it emits:
  - review decision, mergeable / mergeStateStatus
  - CI check rollup (per-check conclusion)
  - unresolved review threads (path:line + last comment author, truncated body)
  - review submissions (author + state)
  - issue-comment authors + truncated bodies

Anything that changes when a reviewer acts changes the output.

Usage: my-open-prs.py [--user LOGIN] [--org ORG] [--json] [--retry-marker]

--user defaults to the authenticated gh login.

--retry-marker (automation only, interactive runs should not pass it): appends
ONE volatile line while at least one PR needs work from the bot itself. The
tick value changes every run, so an output hash keeps changing and the
consuming automation is re-invoked until the bot has acted — a failed run is
retried instead of silently dropped.

"Needs work from the bot" is deliberately keyed on conditions the BOT clears,
never on reviewer-side state:
  - an unresolved review thread whose LAST comment is not by the bot
  - CI rollup FAILURE/ERROR
  - mergeStateStatus BEHIND or DIRTY
reviewDecision=CHANGES_REQUESTED alone is deliberately EXCLUDED: it persists
until a human re-reviews, so keying on it would re-wake the automation every
tick for days while merely awaiting re-review. A bare changes-requested review
with no threads still changes the output once via the normal hash change of
the reviews list; it just does not get the retry loop.

Runaway circuit breaker (--retry-marker mode): per-PR state (path from
$ZPR_PR_STATE_PATH, default ~/.zpr-pr-retry-state.json) tracks failed rounds
(state changed but PR still pending) and stale ticks (pending, zero change).
After ZPR_PR_MAX_ROUNDS (default 3) rounds or ZPR_PR_MAX_STALE_TICKS (default
12) the PR is dropped from the volatile marker and a STABLE
ESCALATED-REVIEW-WORK line is emitted instead: one hash change -> one
escalation to a human -> idle. Resets automatically when the PR stops needing
bot work; manual reset = delete the state file.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ghretry import gh_login, run_gh  # noqa: E402

DEFAULT_ORG = "org-zpr"
BODY_CLIP = 160

PR_QUERY = """
query($owner:String!, $repo:String!, $num:Int!) {
  repository(owner:$owner, name:$repo) {
    pullRequest(number:$num) {
      number
      title
      isDraft
      state
      reviewDecision
      mergeable
      mergeStateStatus
      reviews(first:50) {
        nodes { author { login } state }
      }
      comments(first:100) {
        nodes { author { login } body }
      }
      reviewThreads(first:100) {
        nodes {
          isResolved
          isOutdated
          path
          line
          comments(first:20) { nodes { author { login } body } }
        }
      }
      commits(last:1) {
        nodes {
          commit {
            oid
            statusCheckRollup {
              state
              contexts(first:50) {
                nodes {
                  __typename
                  ... on CheckRun { name conclusion status }
                  ... on StatusContext { context state }
                }
              }
            }
          }
        }
      }
    }
  }
}
"""


def clip(text: str | None) -> str:
    if not text:
        return ""
    flat = " ".join(text.split())
    return flat[:BODY_CLIP] + ("..." if len(flat) > BODY_CLIP else "")


def login(node: dict | None) -> str:
    if not node:
        return "?"
    return (node.get("author") or {}).get("login") or "?"


def list_open_prs(user: str, org: str) -> list[dict]:
    out = run_gh(
        [
            "search", "prs",
            "--author", user,
            "--owner", org,
            "--state", "open",
            "--limit", "100",
            "--json", "number,title,repository,url",
        ]
    )
    prs = json.loads(out or "[]")
    return sorted(prs, key=lambda p: (p["repository"]["name"], p["number"]))


def fetch_pr(repo: str, num: int, org: str) -> dict:
    out = run_gh(
        [
            "api", "graphql",
            "-f", f"query={PR_QUERY}",
            "-f", f"owner={org}",
            "-f", f"repo={repo}",
            "-F", f"num={num}",
        ]
    )
    return json.loads(out)["data"]["repository"]["pullRequest"]


def summarize(repo: str, url: str, pr: dict) -> dict:
    checks: list[str] = []
    commits = (pr.get("commits") or {}).get("nodes") or []
    rollup_state = "NONE"
    if commits:
        rollup = (commits[0]["commit"] or {}).get("statusCheckRollup")
        if rollup:
            rollup_state = rollup.get("state") or "NONE"
            for c in (rollup.get("contexts") or {}).get("nodes") or []:
                if c.get("__typename") == "CheckRun":
                    checks.append(
                        f"{c.get('name')}={c.get('conclusion') or c.get('status')}"
                    )
                else:
                    checks.append(f"{c.get('context')}={c.get('state')}")
    checks.sort()

    unresolved = []
    for t in (pr.get("reviewThreads") or {}).get("nodes") or []:
        if t.get("isResolved"):
            continue
        tc = (t.get("comments") or {}).get("nodes") or []
        last = tc[-1] if tc else None
        unresolved.append(
            {
                "path": t.get("path"),
                "line": t.get("line"),
                "outdated": bool(t.get("isOutdated")),
                "author": login(last),
                "body": clip((last or {}).get("body")),
            }
        )
    unresolved.sort(key=lambda t: (t["path"] or "", t["line"] or 0, t["body"]))

    reviews = sorted(
        f"{login(r)}:{r.get('state')}"
        for r in (pr.get("reviews") or {}).get("nodes") or []
    )
    comments = sorted(
        f"{login(c)}: {clip(c.get('body'))}"
        for c in (pr.get("comments") or {}).get("nodes") or []
    )

    return {
        "repo": repo,
        "number": pr["number"],
        "title": pr["title"],
        "url": url,
        "draft": bool(pr.get("isDraft")),
        "reviewDecision": pr.get("reviewDecision") or "NONE",
        "mergeable": pr.get("mergeable") or "UNKNOWN",
        "mergeStateStatus": pr.get("mergeStateStatus") or "UNKNOWN",
        "checksState": rollup_state,
        "checks": checks,
        "reviews": reviews,
        "unresolvedThreads": unresolved,
        "comments": comments,
    }


def needs_work(item: dict, user: str) -> bool:
    """True when this PR is waiting on an action the BOT itself performs.

    Self-clearing by design: every condition here goes away when the bot
    replies or pushes, so the retry marker cannot loop on reviewer-side state.
    Exact-string caveat: thread authors compare against ``user`` exactly.
    """
    if item.get("draft"):
        return False
    if any(t["author"] != user for t in item.get("unresolvedThreads") or []):
        return True
    if item.get("checksState") in ("FAILURE", "ERROR"):
        return True
    if item.get("mergeStateStatus") in ("BEHIND", "DIRTY"):
        return True
    return False


STATE_PATH = os.environ.get(
    "ZPR_PR_STATE_PATH", os.path.expanduser("~/.zpr-pr-retry-state.json")
)
# Circuit breaker thresholds (overridable for tests / tuning):
#   MAX_ROUNDS: distinct PR states observed while continuously pending. Each
#     fingerprint change while still pending means the bot (or someone) acted
#     and the PR STILL needs bot work — after this many failed rounds, stop
#     retrying and escalate to a human instead of iterating forever.
#   MAX_STALE_TICKS: consecutive ticks pending with ZERO observable change —
#     the automation is evidently unable to act at all. At 5-min ticks, 12 == 1 hour.
MAX_ROUNDS = int(os.environ.get("ZPR_PR_MAX_ROUNDS", "3"))
MAX_STALE_TICKS = int(os.environ.get("ZPR_PR_MAX_STALE_TICKS", "12"))


def fingerprint(item: dict) -> str:
    """Stable hash of a summarized PR item (summarize() emits no clock values)."""
    return hashlib.sha256(
        json.dumps(item, sort_keys=True).encode()
    ).hexdigest()[:16]


def _load_state(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _save_state(path: str, state: dict) -> None:
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or ".", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(state, fh, indent=1, sort_keys=True)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def update_breaker(
    items: list[dict],
    user: str,
    state_path: str = STATE_PATH,
    max_rounds: int | None = None,
    max_stale: int | None = None,
) -> tuple[list[dict], list[str]]:
    """Runaway circuit breaker for the retry marker.

    Tracks, per pending PR, how many distinct states (rounds) it has gone
    through while continuously needing bot work, and how many consecutive
    ticks it has sat pending with no change at all. Either limit trips the
    breaker: the PR is removed from the volatile retry marker and a STABLE
    ESCALATED-REVIEW-WORK line (frozen counters, no clock) is emitted
    instead — the hash changes once, the escalation happens once, then idle.

    State for a PR is deleted the moment it no longer needs bot work, which
    also resets a tripped breaker. Manual reset: delete the state file.

    Returns (still_retryable_items, escalated_lines) and persists state.
    """
    if max_rounds is None:
        max_rounds = MAX_ROUNDS
    if max_stale is None:
        max_stale = MAX_STALE_TICKS
    state = _load_state(state_path)
    active: list[dict] = []
    escalated: list[str] = []
    new_state: dict = {}
    for it in items:
        if not needs_work(it, user):
            continue  # pending cleared -> old entry (if any) is dropped
        key = f"{it['repo']}#{it['number']}"
        fp = fingerprint(it)
        st = state.get(key)
        if not isinstance(st, dict):
            st = {"fp": fp, "rounds": 0, "stale": 0}
        elif st.get("fp") != fp:
            st = {
                "fp": fp,
                "rounds": int(st.get("rounds", 0)) + 1,
                "stale": 0,
                "frozen": st.get("frozen"),
            }
        else:
            st = dict(st)
            st["stale"] = int(st.get("stale", 0)) + 1
        tripped = (
            st.get("frozen") is not None
            or st["rounds"] >= max_rounds
            or st["stale"] >= max_stale
        )
        if tripped:
            if st.get("frozen") is None:
                st["frozen"] = f"rounds={st['rounds']} stale={st['stale']}"
            escalated.append(
                f"ESCALATED-REVIEW-WORK {key} [{st['frozen']}]"
                "  (circuit breaker tripped: repeated bot rounds did not clear"
                " this PR — do NOT work it again; escalate to a human. Resets"
                " automatically when the PR no longer needs bot work.)"
            )
        else:
            active.append(it)
        new_state[key] = st
    escalated.sort()
    try:
        _save_state(state_path, new_state)
    except OSError as exc:  # diagnostics to stderr; stdout hash must stay clean
        print(f"warning: could not persist breaker state: {exc}", file=sys.stderr)
    return active, escalated


def retry_marker(items: list[dict], user: str) -> str | None:
    """Volatile line emitted only while at least one PR needs bot work.

    Deliberately unstable (tick=epoch): defeats hash suppression so a failed
    run is retried next tick. Absent when idle, so idle output stays
    byte-stable. Do not \"fix\" the timestamp.
    """
    pending = [it for it in items if needs_work(it, user)]
    if not pending:
        return None
    ids = ",".join(f"{it['repo']}#{it['number']}" for it in pending)
    tick = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
    return (
        f"PENDING-REVIEW-WORK {len(pending)} [{ids}] tick={tick}"
        "  (volatile line: forces re-check until the bot has replied/pushed;"
        " not a change signal)"
    )


def render(items: list[dict], user: str, org: str) -> str:
    if not items:
        return f"No open PRs authored by {user} in {org}."
    lines: list[str] = [f"Open PRs authored: {len(items)}"]
    for it in items:
        ready = (
            it["reviewDecision"] == "APPROVED"
            and not it["unresolvedThreads"]
            and it["mergeable"] == "MERGEABLE"
            and it["mergeStateStatus"] == "CLEAN"
        )
        lines.append("")
        lines.append(f"== {it['repo']}#{it['number']} {it['title']}")
        lines.append(f"   url: {it['url']}")
        lines.append(
            f"   draft={it['draft']} review={it['reviewDecision']} "
            f"mergeable={it['mergeable']} mergeState={it['mergeStateStatus']} "
            f"checks={it['checksState']}"
        )
        lines.append(f"   READY_FOR_HUMAN_MERGE={ready}")
        if it["checks"]:
            lines.append("   checks: " + ", ".join(it["checks"]))
        if it["reviews"]:
            lines.append("   reviews: " + ", ".join(it["reviews"]))
        if it["unresolvedThreads"]:
            lines.append(f"   unresolved threads: {len(it['unresolvedThreads'])}")
            for t in it["unresolvedThreads"]:
                flag = " (outdated)" if t["outdated"] else ""
                lines.append(
                    f"     - {t['path']}:{t['line']}{flag} [{t['author']}] {t['body']}"
                )
        else:
            lines.append("   unresolved threads: 0")
        if it["comments"]:
            lines.append(f"   comments: {len(it['comments'])}")
            for c in it["comments"]:
                lines.append(f"     - {c}")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--user", default=None,
                    help="PR author login (default: authenticated gh user)")
    ap.add_argument("--org", default=DEFAULT_ORG)
    ap.add_argument("--json", action="store_true", dest="as_json")
    ap.add_argument("--retry-marker", action="store_true", dest="marker")
    args = ap.parse_args()
    user = args.user or gh_login()

    items = []
    for pr in list_open_prs(user, args.org):
        repo = pr["repository"]["name"]
        detail = fetch_pr(repo, pr["number"], args.org)
        items.append(summarize(repo, pr["url"], detail))

    if args.as_json:
        print(json.dumps(items, indent=2, sort_keys=True))
    else:
        print(render(items, user, args.org))
    if args.marker:
        # Circuit breaker: PRs that keep needing bot work despite repeated
        # rounds (or sit pending with no change for too long) are pulled out
        # of the volatile retry loop and reported on a STABLE escalation line
        # instead — one hash change, one escalation, then silence.
        retryable, escalated = update_breaker(items, user)
        for line in escalated:
            print(line)
        line = retry_marker(retryable, user)
        if line:
            print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
