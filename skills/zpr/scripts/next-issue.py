#!/usr/bin/env python3
"""Print the next issue to work on in mkolehmainen/zipline, and the rest of the ready set.

An issue is READY when it is open, every issue in its GitHub native `blockedBy`
dependency list is closed, and it is UNASSIGNED. The NEXT issue is the ready
issue that comes first in the umbrella's sub-issue list, which is maintained in
execution order (see "Picking the next issue" in ../SKILL.md) -- so position in
that list already encodes critical-path-first and no separate tiebreak is
needed.

An assigned issue is treated as UNDERWAY, not ready: pickup step 3 assigns the
issue before branching, so assignment is the marker that someone already holds
it. Without this an unattended agent re-picks the issue it is already working
-- an open issue with an open PR still has all its blockers closed. Underway
issues are reported separately so they can be polled instead of picked up.

This reads state and changes nothing.

Usage:
  python3 next-issue.py           # human-readable
  python3 next-issue.py --json    # {"next": {...}, "ready": [...], "underway": [...]}

Requires: gh authenticated with the `repo` scope. Dependencies and sub-issues are
repository data, so no project scope is needed here.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ghretry import run_gh  # noqa: E402

OWNER = "mkolehmainen"
REPO = "zipline"
UMBRELLA = 1  # tracking issue; never itself a work item

QUERY = """
query($owner:String!, $repo:String!, $cursor:String) {
  repository(owner:$owner, name:$repo) {
    issues(first:100, states:OPEN, after:$cursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title url
        labels(first:10) { nodes { name } }
        assignees(first:5) { nodes { login } }
        blockedBy(first:50) { nodes { number state } }
      }
    }
  }
}
"""


def gh_graphql(query, **variables):
    """GraphQL call with bounded retries on transient network faults."""
    cmd = ["api", "graphql", "-f", f"query={query}"]
    for k, v in variables.items():
        if v is None:
            continue
        flag = "-F" if isinstance(v, int) else "-f"
        cmd += [flag, f"{k}={v}"]
    d = json.loads(run_gh(cmd))
    if "errors" in d:
        raise SystemExit("GraphQL error: " + json.dumps(d["errors"], indent=2))
    return d


def open_issues():
    """Every open issue in the tracker, with its blocked-by list."""
    cursor, out = None, []
    while True:
        page = gh_graphql(QUERY, owner=OWNER, repo=REPO, cursor=cursor)
        page = page["data"]["repository"]["issues"]
        out.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return out
        cursor = page["pageInfo"]["endCursor"]


def execution_order():
    """Issue numbers in umbrella sub-issue order == intended execution order.

    Anything not attached to the umbrella sorts after everything that is.
    """
    raw = run_gh(["api", f"repos/{OWNER}/{REPO}/issues/{UMBRELLA}/sub_issues",
                  "--paginate", "-q", ".[].number"])
    return {int(n): i for i, n in enumerate(raw.split())}


def summarize(issue, order):
    """Flatten one GraphQL issue node into the shape this script reports."""
    return {
        "number": issue["number"],
        "title": issue["title"],
        "url": issue["url"],
        "repo_label": ",".join(l["name"] for l in issue["labels"]["nodes"]),
        "assignees": [a["login"] for a in issue["assignees"]["nodes"]],
        "position": order.get(issue["number"], 10_000 + issue["number"]),
    }


def select(issues, order):
    """Split open issues into (ready, underway), both in execution order.

    Ready = unblocked and unassigned, so it is safe to pick up. Underway =
    unblocked but assigned, i.e. already held by someone; poll those instead.
    Blocked issues and the umbrella itself appear in neither list.
    """
    ready, underway = [], []
    for issue in issues:
        if issue["number"] == UMBRELLA:
            continue
        if any(b["state"] == "OPEN" for b in issue["blockedBy"]["nodes"]):
            continue
        row = summarize(issue, order)
        (underway if row["assignees"] else ready).append(row)
    ready.sort(key=lambda r: r["position"])
    underway.sort(key=lambda r: r["position"])
    return ready, underway


def main():
    as_json = "--json" in sys.argv
    order = execution_order()
    ready, underway = select(open_issues(), order)

    if as_json:
        print(json.dumps({"next": ready[0] if ready else None,
                          "ready": ready, "underway": underway}, indent=2))
        return

    if ready:
        nxt = ready[0]
        print(f"NEXT  #{nxt['number']}  [{nxt['repo_label']}]  {nxt['title']}")
        print(f"      {nxt['url']}")
        if len(ready) > 1:
            print(f"\nalso ready ({len(ready) - 1}):")
            for r in ready[1:]:
                print(f"  #{r['number']:<3} [{r['repo_label']}] {r['title']}")
    else:
        print("Nothing ready: every open issue is blocked, assigned, or the tracker is empty.")

    # Assigned-but-unblocked issues are the work in flight. Printed because an
    # issue silently vanishing from the ready set is otherwise baffling.
    if underway:
        print(f"\nunderway, not pickable ({len(underway)}) -- poll these instead:")
        for r in underway:
            print(f"  #{r['number']:<3} [{r['repo_label']}] {r['title']}"
                  f"  ({', '.join(r['assignees'])})")


if __name__ == "__main__":
    main()
