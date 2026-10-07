#!/usr/bin/env python3
"""Print the next issue to work on in mkolehmainen/zipline, and the rest of the ready set.

An issue is PICKABLE when all four hold (zipline#155):

    pickable = open AND unassigned AND all blockers closed AND board Status == Ready

The first three come from the tracker (GitHub native `blockedBy` dependencies
and assignment); the fourth comes from the project board -- user-owned project
#1 under mkolehmainen -- and is OPERATOR-OWNED: nothing promotes an issue to
`Ready` mechanically (board-sync.py only suggests), so pickup is opt-in per
issue. An issue absent from the board, or on the board with no Status value,
counts as Backlog and is not pickable (operator decision on zipline#155).

The dependency graph is still enforced independently of Ready: marking a
blocked issue Ready is *pre-authorization* -- it is held while any blocker is
open and picked up automatically on the tick after the last blocker closes.
Those are reported under `pre-authorized` (intended state, not a warning).
Unblocked, unassigned issues still sitting in Backlog are reported under
`awaiting-ready` every run so a forgotten green-light shows up instead of
silently never starting.

The NEXT issue is the pickable issue that comes first in its umbrella's
sub-issue list, which is maintained in execution order (see "Picking the next
issue" in ../SKILL.md) -- so position in that list already encodes
critical-path-first and no separate tiebreak is needed.

An assigned issue is treated as UNDERWAY, not ready: pickup step 3 assigns the
issue before branching, so assignment is the marker that someone already holds
it. Without this an unattended agent re-picks the issue it is already working
-- an open issue with an open PR still has all its blockers closed. Underway
issues are reported separately so they can be polled instead of picked up.

An UMBRELLA -- an issue that has sub-issues -- is a container for work, not
work itself, so it is never reported as pickable. Umbrellas are *derived* from
the sub-issue graph rather than listed here: the tracker holds one umbrella per
feature and filing the next one must not require editing this script.

SECOND SOURCE: org-zpr/zipline, gated by org project #5 "zipline" (operator
decision, 2026-10-07). Only issues from the org-zpr/zipline repository are
candidates -- the org project also carries legacy org-zpr/zpr-* issues, which
are never candidates and whose repositories must never be touched;
implementation always lands in the mkolehmainen/zl-* forks. The predicate
differs from the tracker's in two ways:

    pickable(org) = open AND all blockers closed AND Status == Ready
                    AND Iteration == current
                    AND (unassigned OR assigned solely to the bot)

- The Iteration field gates by time: only current-iteration items are picked.
  Ready in a future iteration is pre-authorized (auto-starts when it begins);
  Ready in an iteration that has already ENDED is reported separately under
  `expired_iteration` -- it can never auto-start, so it needs rescheduling.
- Explicit assignment to the bot is an OPT-IN, not an underway marker, so it
  does not suppress pickup by itself. The underway marker for org issues is
  board Status `In progress`/`In review` (pickup sets it at claim time, same
  as the tracker flow). An issue assigned to anyone else is theirs: never
  picked, never reported as ours.

mkolehmainen/zipline issues take precedence: every tracker-ready issue sorts
before every org-ready one. Among org candidates the tiebreak is lowest issue
number (the org project has no ordering field).

This reads state and changes nothing.

Usage:
  python3 next-issue.py           # human-readable
  python3 next-issue.py --json    # {"next": {...}, "ready": [...], "pre_authorized": [...],
                                  #  "awaiting_ready": [...], "underway": [...],
                                  #  "expired_iteration": [...]}

Requires: gh authenticated with the `repo` scope, plus `read:project` (or
`project`) for the board Status read.
"""
import datetime
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ghretry import run_gh  # noqa: E402

OWNER = "mkolehmainen"
REPO = "zipline"
PROJECT_NUMBER = 1

# Second source (operator decision 2026-10-07): org-zpr/zipline issues, gated
# by the org-owned project #5 "zipline". The bot login is pinned rather than
# read from `gh api user` so a fixture-driven test needs no network and a
# mis-authed gh cannot silently widen "assigned to the bot" to someone else.
ORG_OWNER = "org-zpr"
ORG_REPO = "zipline"
ORG_PROJECT_NUMBER = 5
BOT_LOGIN = "ZprBot1"

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

# Board Status per issue. The board is user-owned, so the root field must be
# `user(login:)` -- `organization(login:)` returns null for it. The content's
# repository is fetched because the board also carries legacy issues from
# `org-zpr/zpr-*` repositories: an item is only this tracker's if it lives in
# OWNER/REPO, and keying by bare number would let a same-numbered foreign item
# overwrite a tracker issue's Status (PR #47 review).
BOARD_QUERY = """
query($owner:String!, $number:Int!, $cursor:String) {
  user(login:$owner) { projectV2(number:$number) {
    items(first:100, after:$cursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        content { ... on Issue { number repository { nameWithOwner } } }
        fieldValueByName(name:"Status") {
          ... on ProjectV2ItemFieldSingleSelectValue { name }
        }
      }
    }
  } }
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


def board_statuses():
    """Map issue number -> board Status name, from user-owned project #1.

    Only `OWNER/REPO` (tracker) items are mapped: the board also carries
    legacy `org-zpr/zpr-*` issues, and a foreign item sharing a number with a
    tracker issue must not overwrite its Status -- an unrelated Ready item
    would bypass the operator gate, an unrelated Backlog item would suppress
    an authorized issue (PR #47 review).

    An issue absent from this map, or mapped to None (item exists but Status is
    unset), counts as Backlog in `select` -- the operator's Q1 answer on
    zipline#155. Field values live on the board item, not the issue, so this is
    re-read every run rather than cached.
    """
    tracker = f"{OWNER}/{REPO}"
    cursor, statuses = None, {}
    while True:
        page = gh_graphql(BOARD_QUERY, owner=OWNER, number=PROJECT_NUMBER, cursor=cursor)
        items = page["data"]["user"]["projectV2"]["items"]
        for node in items["nodes"]:
            content = node.get("content") or {}
            number = content.get("number")
            if number is None:  # draft items and PRs have no issue number
                continue
            if (content.get("repository") or {}).get("nameWithOwner") != tracker:
                continue  # legacy upstream issue: not this tracker's number
            value = node.get("fieldValueByName") or {}
            statuses[number] = value.get("name")
        if not items["pageInfo"]["hasNextPage"]:
            return statuses
        cursor = items["pageInfo"]["endCursor"]


# Org project #5 items, with everything the org predicate needs in one query:
# the issue's own state/assignees/blockers/sub-issues plus the item's Status
# and Iteration values. The project is org-owned, so the root field is
# `organization(login:)` -- the mirror image of the user-owned board above.
ORG_BOARD_QUERY = """
query($owner:String!, $number:Int!, $cursor:String) {
  organization(login:$owner) { projectV2(number:$number) {
    items(first:100, after:$cursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        content { ... on Issue {
          number title url state
          repository { nameWithOwner }
          labels(first:10) { nodes { name } }
          assignees(first:5) { nodes { login } }
          blockedBy(first:50) { nodes { number state } }
          subIssues(first:100) { nodes { number } }
        } }
        status: fieldValueByName(name:"Status") {
          ... on ProjectV2ItemFieldSingleSelectValue { name }
        }
        iteration: fieldValueByName(name:"Iteration") {
          ... on ProjectV2ItemFieldIterationValue { startDate duration title }
        }
      }
    }
  } }
}
"""


def org_items():
    """Every item on org project #5, raw."""
    cursor, out = None, []
    while True:
        page = gh_graphql(ORG_BOARD_QUERY, owner=ORG_OWNER,
                          number=ORG_PROJECT_NUMBER, cursor=cursor)
        items = page["data"]["organization"]["projectV2"]["items"]
        out.extend(items["nodes"])
        if not items["pageInfo"]["hasNextPage"]:
            return out
        cursor = items["pageInfo"]["endCursor"]


def iteration_is_current(value, today):
    """True when `today` falls inside the item's Iteration value.

    The value carries its own startDate and duration (days), so currency is
    computed from the item -- no second query for the field's configuration.
    No Iteration value means not scheduled, which is not current.
    """
    if not value or not value.get("startDate"):
        return False
    start, end = iteration_bounds(value)
    return start <= today < end


def iteration_bounds(value):
    """(first day, first day AFTER) of an Iteration field value.

    The end is exclusive: a 14-day iteration starting 2026-09-29 covers
    2026-09-29 .. 2026-10-12 and returns end 2026-10-13.
    """
    start = datetime.date.fromisoformat(value["startDate"])
    return start, start + datetime.timedelta(days=int(value["duration"]))


def iteration_has_ended(value, today):
    """True when the item's Iteration lies wholly in the past (PR #61 review).

    Distinguishes an expired schedule from a scheduled-ahead one: both are
    "not current", but only a future iteration can ever become current, so
    only that one may be reported as auto-starting. No Iteration value means
    unscheduled, which has not ended.
    """
    if not value or not value.get("startDate"):
        return False
    _, end = iteration_bounds(value)
    return today >= end


def summarize_org(content):
    """Flatten one org project item's issue into the reported row shape.

    Org rows sort after every tracker row (tracker precedence, operator
    decision 2026-10-07) and among themselves by issue number -- the org
    project has no ordering field.
    """
    return {
        "number": content["number"],
        "title": content["title"],
        "url": content["url"],
        "repo_label": ",".join(l["name"] for l in content["labels"]["nodes"]),
        "assignees": [a["login"] for a in content["assignees"]["nodes"]],
        "position": 100_000 + content["number"],
        "tracker": f"{ORG_OWNER}/{ORG_REPO}",
    }


def select_org(items, today, bot=BOT_LOGIN):
    """Split org project items into (ready, pre_authorized, awaiting_ready,
    underway, expired_iteration), each sorted by issue number.

    pickable(org) = open AND all blockers closed AND Status == Ready
                    AND Iteration == current
                    AND (unassigned OR assigned solely to `bot`)

    Differences from the tracker's `select`, both deliberate:

    - Assignment to the bot is an opt-in, not an underway marker, so it does
      not suppress pickup. Underway is marked by board Status `In progress` /
      `In review`, which pickup sets at claim time. An issue assigned to
      anyone else is theirs -- skipped entirely, never reported.
    - The Iteration field gates by time. Ready in a FUTURE iteration is
      scheduled-ahead: reported under pre_authorized (with `held_by`) so it
      cannot rot silently, picked up on the tick after its iteration starts.
      Ready in an iteration that has already ENDED can never auto-start, so
      it goes to expired_iteration instead (row carries `iteration`, the
      ended iteration's title, plus `blocked_by`): it needs the operator to
      reschedule it, and saying "auto-starts" would hide that (PR #61 review).
      Ready with no Iteration value is unscheduled and stays pre_authorized
      with `held_by` "iteration (none scheduled)".

    Only org-zpr/zipline issues are candidates: the project also carries
    legacy org-zpr/zpr-* issues, which are never work for this agent (their
    repositories are never touched; implementation lands in the
    mkolehmainen/zl-* forks).
    """
    wanted = f"{ORG_OWNER}/{ORG_REPO}"
    ready, pre_authorized, awaiting_ready, underway = [], [], [], []
    expired = []
    for item in items:
        content = item.get("content") or {}
        if content.get("number") is None:  # drafts and PRs
            continue
        if (content.get("repository") or {}).get("nameWithOwner") != wanted:
            continue  # legacy org-zpr/zpr-* issue: never a candidate
        if content["state"] != "OPEN":
            continue
        if content["subIssues"]["nodes"]:
            continue  # umbrella: a container for work, not work
        assignees = [a["login"] for a in content["assignees"]["nodes"]]
        if assignees and assignees != [bot]:
            continue  # someone else's issue: theirs, not reported
        status = (item.get("status") or {}).get("name")
        row = summarize_org(content)
        if status in ("In progress", "In review"):
            underway.append(row)
            continue
        if status != "Ready":
            # Backlog / Done / unset: held by the operator gate. Only the
            # unblocked ones are reported -- same rule as the tracker.
            open_blockers = [b["number"] for b in content["blockedBy"]["nodes"]
                             if b["state"] == "OPEN"]
            if status in (None, "Backlog") and not open_blockers:
                awaiting_ready.append(row)
            continue
        open_blockers = sorted(b["number"] for b in content["blockedBy"]["nodes"]
                               if b["state"] == "OPEN")
        iteration = item.get("iteration")
        if iteration_has_ended(iteration, today):
            # Can never become current again: report for rescheduling, never
            # as pre-authorized and never as pickable.
            row["blocked_by"] = open_blockers
            row["iteration"] = iteration.get("title") or iteration["startDate"]
            expired.append(row)
            continue
        current = iteration_is_current(iteration, today)
        if open_blockers or not current:
            row["blocked_by"] = open_blockers
            if not current:
                it = iteration or {}
                row["held_by"] = f"iteration ({it.get('title') or 'none scheduled'})"
            pre_authorized.append(row)
            continue
        ready.append(row)
    for rows in (ready, pre_authorized, awaiting_ready, underway, expired):
        rows.sort(key=lambda r: r["position"])
    return ready, pre_authorized, awaiting_ready, underway, expired


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


def select(issues, order, statuses):
    """Split issues into (ready, pre_authorized, awaiting_ready, underway),
    each in execution order.

    ready           open + unassigned + unblocked + board Status Ready:
                    pickable now.
    pre_authorized  Ready but blocked: the operator has green-lit it ahead of
                    time; it auto-starts on the tick after its last blocker
                    closes. Rows carry `blocked_by`, the open blocker numbers.
                    Intended state, not a warning.
    awaiting_ready  open + unassigned + unblocked, but board Status is not
                    Ready: held by the gate until the operator flips it.
                    Reported every run so a forgotten green-light is visible.
    underway        open + unblocked but assigned: already held by someone;
                    poll it instead. Assignment trumps board Status -- the
                    gate governs pickup, not work already claimed.

    `statuses` maps issue number -> board Status name; an issue missing from
    it, or mapped to None, counts as Backlog (not pickable) per the operator's
    Q1 answer on zipline#155. Closed issues, umbrellas, and blocked non-Ready
    issues appear in no list.
    """
    tracking = umbrellas(issues)
    ready, pre_authorized, awaiting_ready, underway = [], [], [], []
    for issue in issues:
        if issue["state"] != "OPEN":
            continue
        if issue["number"] in tracking:
            continue
        open_blockers = [b["number"] for b in issue["blockedBy"]["nodes"]
                         if b["state"] == "OPEN"]
        row = summarize(issue, order)
        if row["assignees"]:
            if not open_blockers:
                underway.append(row)
            continue
        is_ready = statuses.get(issue["number"]) == "Ready"
        if open_blockers:
            if is_ready:
                row["blocked_by"] = sorted(open_blockers)
                pre_authorized.append(row)
            continue
        (ready if is_ready else awaiting_ready).append(row)
    for rows in (ready, pre_authorized, awaiting_ready, underway):
        rows.sort(key=lambda r: r["position"])
    return ready, pre_authorized, awaiting_ready, underway


def ref(row):
    """Short reference for a row: `#N` for tracker issues, `org-zpr/zipline#N`
    for org-source rows, so the two sources can never be confused in output."""
    tracker = row.get("tracker")
    return f"{tracker}#{row['number']}" if tracker else f"#{row['number']}"


def main():
    as_json = "--json" in sys.argv
    issues = all_issues()
    order = execution_order(issues)
    statuses = board_statuses()
    ready, pre_authorized, awaiting_ready, underway = select(issues, order, statuses)

    # Second source: org-zpr/zipline via org project #5. Org rows carry
    # position >= 100_000, so after the merged sort every tracker row precedes
    # every org row -- mkolehmainen/zipline takes precedence (operator
    # decision 2026-10-07).
    o_ready, o_pre, o_awaiting, o_underway, expired_iteration = select_org(
        org_items(), datetime.date.today())
    ready += o_ready
    pre_authorized += o_pre
    awaiting_ready += o_awaiting
    underway += o_underway
    for rows in (ready, pre_authorized, awaiting_ready, underway):
        rows.sort(key=lambda r: r["position"])

    if as_json:
        print(json.dumps({"next": ready[0] if ready else None,
                          "ready": ready, "pre_authorized": pre_authorized,
                          "awaiting_ready": awaiting_ready,
                          "underway": underway,
                          "expired_iteration": expired_iteration}, indent=2))
        return

    if ready:
        nxt = ready[0]
        print(f"NEXT  {ref(nxt)}  [{nxt['repo_label']}]  {nxt['title']}")
        print(f"      {nxt['url']}")
        if len(ready) > 1:
            print(f"\nalso ready ({len(ready) - 1}):")
            for r in ready[1:]:
                print(f"  {ref(r):<4} [{r['repo_label']}] {r['title']}")
    else:
        print("Nothing pickable: every open issue is blocked, assigned, "
              "or awaiting the operator's Ready on the board.")

    # Ready-but-blocked is intended state: the operator pre-authorized it and
    # it starts by itself when the last blocker closes (or, for org items,
    # when the scheduled iteration becomes current).
    if pre_authorized:
        print(f"\npre-authorized ({len(pre_authorized)}) -- "
              f"auto-starts when blockers close:")
        for r in pre_authorized:
            holds = [f"blocked by {', '.join(f'#{n}' for n in r['blocked_by'])}"
                     ] if r.get("blocked_by") else []
            if r.get("held_by"):
                holds.append(f"held by {r['held_by']}")
            print(f"  {ref(r):<4} [{r['repo_label']}] {r['title']}"
                  f"  ({'; '.join(holds)})")

    # Ready, but its Iteration has already ended (org source only). Unlike
    # pre-authorized rows, nothing will ever start these: the operator has to
    # move them into the current or a future iteration (PR #61 review).
    if expired_iteration:
        print(f"\nReady, but its iteration ended -- reschedule "
              f"({len(expired_iteration)}); never auto-starts:")
        for r in expired_iteration:
            notes = [f"iteration {r['iteration']} ended"]
            if r.get("blocked_by"):
                notes.append(f"blocked by {', '.join(f'#{n}' for n in r['blocked_by'])}")
            print(f"  {ref(r):<4} [{r['repo_label']}] {r['title']}"
                  f"  ({'; '.join(notes)})")

    # Unblocked but not Ready: the gate is holding these for the operator.
    # Reported every run so a forgotten green-light cannot rot silently.
    if awaiting_ready:
        print(f"\nawaiting your Ready ({len(awaiting_ready)}) -- "
              f"unblocked and unassigned, but board Status is not Ready:")
        for r in awaiting_ready:
            print(f"  {ref(r):<4} [{r['repo_label']}] {r['title']}")

    # Assigned-but-unblocked issues are the work in flight. Printed because an
    # issue silently vanishing from the ready set is otherwise baffling.
    if underway:
        print(f"\nunderway, not pickable ({len(underway)}) -- poll these instead:")
        for r in underway:
            print(f"  {ref(r):<4} [{r['repo_label']}] {r['title']}"
                  f"  ({', '.join(r['assignees'])})")


if __name__ == "__main__":
    main()
