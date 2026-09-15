#!/usr/bin/env python3
"""Print the next issue to work on in mkolehmainen/zipline, and the rest of the ready set.

An issue is READY when it is open, every issue in its GitHub native `blockedBy`
dependency list is closed, and it is UNASSIGNED. The NEXT issue is the ready
issue that comes first in its umbrella's sub-issue list, which is maintained in
execution order (see "Picking the next issue" in ../SKILL.md) -- so position in
that list already encodes critical-path-first and no separate tiebreak is
needed.

An assigned issue is treated as UNDERWAY, not ready: pickup step 3 assigns the
issue before branching, so assignment is the marker that someone already holds
it. Without this an unattended agent re-picks the issue it is already working
-- an open issue with an open PR still has all its blockers closed. Underway
issues are reported separately so they can be polled instead of picked up.

An UMBRELLA -- an issue that has sub-issues -- is a container for work, not
work itself, so it is never reported as pickable. Umbrellas are *derived* from
the sub-issue graph rather than listed here: the tracker holds one umbrella per
feature and filing the next one must not require editing this script.

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

# Every issue in every state, because a *closed* umbrella can still have open
# children -- so the sub-issue order has to be read from closed issues too.
# `select` filters down to the open ones.
QUERY = """
query($owner:String!, $repo:String!, $cursor:String) {
  repository(owner:$owner, name:$repo) {
    issues(first:100, after:$cursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title url state
        labels(first:10) { nodes { name } }
        assignees(first:5) { nodes { login } }
        blockedBy(first:50) { nodes { number state } }
        subIssues(first:100) { nodes { number } }
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


def all_issues():
    """Every issue in the tracker, with its blocked-by and sub-issue lists."""
    cursor, out = None, []
    while True:
        page = gh_graphql(QUERY, owner=OWNER, repo=REPO, cursor=cursor)
        page = page["data"]["repository"]["issues"]
        out.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return out
        cursor = page["pageInfo"]["endCursor"]


def umbrellas(issues):
    """Numbers of the issues that have sub-issues, i.e. the tracking issues.

    Derived, deliberately: an umbrella is recognised by its shape in the
    sub-issue graph, so a newly filed one is excluded from the ready set with
    no change to this script. Hardcoding a number here would silently offer the
    next feature's umbrella to an agent as a task.
    """
    return {issue["number"] for issue in issues if issue["subIssues"]["nodes"]}


def execution_order(issues):
    """Map issue number -> position, from every umbrella's sub-issue list.

    Each umbrella keeps its sub-issues in intended execution order, which is a
    topological sort along that feature's critical path, so position in the
    list is the whole tiebreak. Umbrellas are walked lowest-number first, so an
    earlier feature's tasks precede a later feature's. Anything attached to no
    umbrella sorts after everything that is (see `summarize`).
    """
    order = {}
    for umbrella in sorted(issues, key=lambda i: i["number"]):
        for sub in umbrella["subIssues"]["nodes"]:
            # setdefault, not assignment: an issue reachable from two umbrellas
            # keeps its first (earliest feature's) position rather than moving.
            order.setdefault(sub["number"], len(order))
    return order


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
    """Split issues into (ready, underway), both in execution order.

    Ready = open, unblocked and unassigned, so it is safe to pick up. Underway
    = open and unblocked but assigned, i.e. already held by someone; poll those
    instead. Closed issues, blocked issues and umbrellas appear in neither
    list.
    """
    tracking = umbrellas(issues)
    ready, underway = [], []
    for issue in issues:
        if issue["state"] != "OPEN":
            continue
        if issue["number"] in tracking:
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
    issues = all_issues()
    order = execution_order(issues)
    ready, underway = select(issues, order)

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
