#!/usr/bin/env python3
"""List "mk zl-zpr project" (mkolehmainen project #1) items assigned to a user in the CURRENT iteration.

Current iteration = the iteration in the Iteration field configuration whose
[startDate, startDate+duration) window contains today.

Usage:
  python3 my-current-tasks.py            # human-readable, assignee = authenticated gh user
  python3 my-current-tasks.py --json     # machine-readable (for automation/diffing)
  python3 my-current-tasks.py --user X   # different assignee login
  python3 my-current-tasks.py --retry-marker
        Append a volatile "PENDING-READY ... tick=<epoch>" line IFF at least one
        item has Status READY_STATUS. Intended for output-hash-based monitors that
        suppress a run when the output is unchanged: without the marker, a run
        that fails after the hash was recorded would leave the unstarted item
        unstarted and never retried. With it, output keeps changing every tick
        while one is outstanding. Idle output stays byte-stable.
        Off by default so interactive runs stay clean.

The board is owned by a USER, not an organization, so the queries below use the
`user(login:)` root field. If the board ever moves to an org, swap that for
`organization(login:)` in both queries and change the two `["user"]` response
lookups to `["organization"]`.

Requires: gh authenticated with scopes repo and read:project (the broader
`project` scope also works). read:org is not needed for a user-owned board.
"""
import datetime
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ghretry import gh_login, run_gh  # noqa: E402

OWNER = "mkolehmainen"
PROJECT_NUMBER = 1

# The project's "unblocked, not started" Status value. This board uses
# Backlog / Ready / In progress / In review / Done -- there is no "Todo".
# Renaming the column means changing this one string.
READY_STATUS = "Ready"

QUERY = """
query($owner:String!, $num:Int!, $cursor:String) {
  user(login:$owner) {
    projectV2(number:$num) {
      title
      items(first:100, after:$cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          fieldValues(first:20) {
            nodes {
              __typename
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { name } }
              }
              ... on ProjectV2ItemFieldIterationValue {
                title startDate duration
                field { ... on ProjectV2IterationField { name } }
              }
            }
          }
          content {
            __typename
            ... on Issue {
              number title url state repository { name }
              assignees(first:10) { nodes { login } }
            }
            ... on PullRequest {
              number title url state repository { name }
              assignees(first:10) { nodes { login } }
            }
            ... on DraftIssue { title }
          }
        }
      }
    }
  }
}
"""

ITER_CFG_QUERY = """
query($owner:String!, $num:Int!) {
  user(login:$owner) {
    projectV2(number:$num) {
      field(name:"Iteration") {
        ... on ProjectV2IterationField {
          configuration { iterations { title startDate duration } }
        }
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
    return json.loads(run_gh(cmd))


def current_iteration_title(today=None):
    today = today or datetime.date.today()
    d = gh_graphql(ITER_CFG_QUERY, owner=OWNER, num=PROJECT_NUMBER)
    cfg = d["data"]["user"]["projectV2"]["field"]["configuration"]
    for it in cfg["iterations"]:
        start = datetime.date.fromisoformat(it["startDate"])
        end = start + datetime.timedelta(days=it["duration"])
        if start <= today < end:
            return it["title"], str(start), str(end - datetime.timedelta(days=1))
    return None, None, None


def fetch_items():
    cursor, items = None, []
    while True:
        d = gh_graphql(QUERY, owner=OWNER, num=PROJECT_NUMBER, cursor=cursor)
        page = d["data"]["user"]["projectV2"]["items"]
        items.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return items
        cursor = page["pageInfo"]["endCursor"]


def field_values(node):
    status, iteration = None, None
    for fv in node.get("fieldValues", {}).get("nodes", []):
        fname = (fv.get("field") or {}).get("name")
        if fv.get("__typename") == "ProjectV2ItemFieldSingleSelectValue" and fname == "Status":
            status = fv.get("name")
        elif fv.get("__typename") == "ProjectV2ItemFieldIterationValue" and fname == "Iteration":
            iteration = fv.get("title")
    return status, iteration


def retry_marker(matches):
    """Volatile line emitted only while at least one item is in READY_STATUS.

    Forces an output-hash monitor to differ on every tick so a tick that
    failed to start the work is retried on the next one. Disappears (restoring
    a stable hash) as soon as nothing is unstarted. Note only the exact
    READY_STATUS string fires it — renaming that project column without
    updating the constant silently disables it.
    """
    pending = [m for m in matches if (m.get("status") or "") == READY_STATUS]
    if not pending:
        return None
    ids = ",".join(f"{m['repo']}#{m['number']}" for m in pending)
    tick = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
    return (
        f"PENDING-READY {len(pending)} [{ids}] tick={tick}"
        f"  (volatile line: forces re-check until these leave {READY_STATUS};"
        " not a change signal)"
    )


def main():
    as_json = "--json" in sys.argv
    want_marker = "--retry-marker" in sys.argv
    if "--user" in sys.argv:
        user = sys.argv[sys.argv.index("--user") + 1]
    else:
        user = gh_login()

    cur, start, end = current_iteration_title()
    if cur is None:
        print("No current iteration matches today's date.", file=sys.stderr)
        sys.exit(2)

    matches = []
    for node in fetch_items():
        content = node.get("content") or {}
        assignees = [a["login"] for a in (content.get("assignees") or {}).get("nodes", [])]
        if user not in assignees:
            continue
        status, iteration = field_values(node)
        if iteration != cur:
            continue
        matches.append({
            "number": content.get("number"),
            "title": content.get("title"),
            "url": content.get("url"),
            "state": content.get("state"),
            "repo": (content.get("repository") or {}).get("name"),
            "status": status,
            "iteration": iteration,
            "type": content.get("__typename"),
        })

    matches.sort(key=lambda m: (m["repo"] or "", m["number"] or 0))

    if as_json:
        payload = {
            "iteration": cur, "start": start, "end": end,
            "user": user, "count": len(matches), "items": matches,
        }
        if want_marker:
            payload["retry_marker"] = retry_marker(matches)
        print(json.dumps(payload, indent=2))
        return

    print(f"{cur} ({start} -> {end})  assignee={user}  items={len(matches)}")
    if not matches:
        print("  (nothing assigned)")
    for m in matches:
        print(f"  [{m['status'] or 'no status'}] {m['repo']}#{m['number']} {m['title']}")
        print(f"      {m['url']}")
    if want_marker:
        marker = retry_marker(matches)
        if marker:
            print(marker)


if __name__ == "__main__":
    main()
