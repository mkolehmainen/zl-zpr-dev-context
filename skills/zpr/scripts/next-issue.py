#!/usr/bin/env python3
"""Print the next issue to work on in mkolehmainen/zipline, and the rest of the ready set.

An issue is READY when it is open and every issue in its GitHub native
`blockedBy` dependency list is closed. The NEXT issue is the ready issue that
comes first in the umbrella's sub-issue list, which is maintained in execution
order (see "Picking the next issue" in ../SKILL.md) -- so position in that list
already encodes critical-path-first and no separate tiebreak is needed.

This reads state and changes nothing.

Usage:
  python3 next-issue.py           # human-readable
  python3 next-issue.py --json    # machine-readable: {"next": {...}, "ready": [...]}

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


def main():
    as_json = "--json" in sys.argv
    order = execution_order()

    ready = []
    for i in open_issues():
        if i["number"] == UMBRELLA:
            continue
        blockers = [b["number"] for b in i["blockedBy"]["nodes"] if b["state"] == "OPEN"]
        if blockers:
            continue
        ready.append({
            "number": i["number"],
            "title": i["title"],
            "url": i["url"],
            "repo_label": ",".join(l["name"] for l in i["labels"]["nodes"]),
            "position": order.get(i["number"], 10_000 + i["number"]),
        })

    ready.sort(key=lambda r: r["position"])

    if as_json:
        print(json.dumps({"next": ready[0] if ready else None, "ready": ready}, indent=2))
        return

    if not ready:
        print("Nothing ready: every open issue is blocked, or the tracker is empty.")
        return
    nxt = ready[0]
    print(f"NEXT  #{nxt['number']}  [{nxt['repo_label']}]  {nxt['title']}")
    print(f"      {nxt['url']}")
    if len(ready) > 1:
        print(f"\nalso ready ({len(ready) - 1}):")
        for r in ready[1:]:
            print(f"  #{r['number']:<3} [{r['repo_label']}] {r['title']}")


if __name__ == "__main__":
    main()
