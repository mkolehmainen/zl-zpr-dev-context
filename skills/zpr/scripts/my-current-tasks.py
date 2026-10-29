#!/usr/bin/env python3
"""List `zl-zpr` board (org-zpr project #6) items assigned to a user in the CURRENT iteration.

Current iteration = the iteration in the Iteration field configuration whose
[startDate, startDate+duration) window contains today. A board with no
Iteration field (project #6 has none) has no iteration to filter on, so every
item assigned to the user is listed.

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

The board is owned by an ORGANIZATION (it moved from the personal account's
project #1 on 2026-10-29), so the queries below use the `organization(login:)`
root field -- `user(login:)` returns null for an org-owned project.

Requires: gh authenticated with scopes repo and read:project (the broader
`project` scope also works).
"""
import datetime
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ghretry import gh_login, run_gh  # noqa: E402

OWNER = "org-zpr"
PROJECT_NUMBER = 6

# The project's "unblocked, not started" Status value. This board uses
# Backlog / Ready / In progress / In review / Done -- there is no "Todo".
# Renaming the column means changing this one string.
READY_STATUS = "Ready"

QUERY = """
query($owner:String!, $num:Int!, $cursor:String) {
  organization(login:$owner) {
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
  organization(login:$owner) {
    projectV2(number:$num) {
      # Not a by-name `field` lookup: on a board without an Iteration field
      # (project #6) GitHub fails the whole query instead of returning null.
      fields(first:50) {
        nodes {
          ... on ProjectV2IterationField {
            name
            configuration { iterations { title startDate duration } }
          }
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


def project_of(payload):
    """The projectV2 object out of a QUERY / ITER_CFG_QUERY response."""
    return payload["data"]["organization"]["projectV2"]


def iteration_config(payload):
    """The Iteration field's iterations, or None if the board has no such field."""
    for field in project_of(payload)["fields"]["nodes"]:
        if field and field.get("name") == "Iteration" and field.get("configuration"):
            return field["configuration"]["iterations"]
    return None


def current_iteration(iterations, today):
    """(title, first day, last day) of the iteration containing today, else Nones."""
    for it in iterations:
        start = datetime.date.fromisoformat(it["startDate"])
        end = start + datetime.timedelta(days=it["duration"])
        if start <= today < end:
            return it["title"], str(start), str(end - datetime.timedelta(days=1))
    return None, None, None


def fetch_items():
    cursor, items = None, []
    while True:
        d = gh_graphql(QUERY, owner=OWNER, num=PROJECT_NUMBER, cursor=cursor)
        page = project_of(d)["items"]
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


def select_matches(nodes, user, iteration):
    """Items assigned to `user`; restricted to `iteration` unless it is None
    (None = the board has no Iteration field, so nothing to restrict by)."""
    matches = []
    for node in nodes:
        content = node.get("content") or {}
        assignees = [a["login"] for a in (content.get("assignees") or {}).get("nodes", [])]
        if user not in assignees:
            continue
        status, item_iteration = field_values(node)
        if iteration is not None and item_iteration != iteration:
            continue
        matches.append({
            "number": content.get("number"),
            "title": content.get("title"),
            "url": content.get("url"),
            "state": content.get("state"),
            "repo": (content.get("repository") or {}).get("name"),
            "status": status,
            "iteration": item_iteration,
            "type": content.get("__typename"),
        })
    return matches


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

    iterations = iteration_config(
        gh_graphql(ITER_CFG_QUERY, owner=OWNER, num=PROJECT_NUMBER))
    if iterations is None:
        cur, start, end = None, None, None  # no Iteration field: no filter
    else:
        cur, start, end = current_iteration(iterations, datetime.date.today())
        if cur is None:
            print("No current iteration matches today's date.", file=sys.stderr)
            sys.exit(2)

    matches = select_matches(fetch_items(), user, cur)
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

    window = f"{cur} ({start} -> {end})" if cur else "no Iteration field (all assigned)"
    print(f"{window}  assignee={user}  items={len(matches)}")
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
