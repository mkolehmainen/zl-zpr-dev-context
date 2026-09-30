#!/usr/bin/env python3
"""Reconcile the `mk zl-zpr project` board with the dependency graph.

Most of the board is documentation derived from two sources of truth -- GitHub
native `blockedBy` edges and each umbrella's sub-issue order. This script
reconciles what is still mechanically derivable and *reports* what is not:

  Iteration  Every open item lands in the current iteration if it has none.
             Existing values are left alone. This is the only field this
             script writes.
  Status     READ-ONLY since zipline#155: `Ready` is operator-owned, because
             pickup is gated on it (pickable = open + unassigned + unblocked
             + Status Ready, see next-issue.py). Backlog->Ready promotion is
             therefore SUGGEST-ONLY -- candidates (open, unassigned,
             unblocked, still Backlog) are printed, and never written, not
             even under --apply. A Ready issue that gained an open blocker is
             pre-authorization (it auto-starts when the blockers close), so
             it is reported, never demoted. `In progress`, `In review` and
             `Done` are owned by whoever is doing the work, as before.

An assigned issue sitting at Backlog or Ready is drift this script refuses to
guess at -- the workflow should have moved it to `In progress` -- so it is
reported and skipped rather than assigned a status by machine.

Reads state and prints a plan by default. Pass --apply to write (Iteration
backfill only).

Usage:
  python3 board-sync.py              # dry run: print the plan
  python3 board-sync.py --apply      # write it
  python3 board-sync.py --json       # the plan as JSON

Requires: `gh` authenticated with `repo` and `project` (or `read:project` for a
dry run). The board is user-owned, so GraphQL must use the `user(login:)` root
field -- `organization(login:)` returns null for it.
"""

from __future__ import annotations

import argparse
import datetime
import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from ghretry import run_gh  # noqa: E402

# The dependency rules live in next-issue.py and are not duplicated here. Its
# filename has a hyphen, so it cannot be imported by name.
_spec = importlib.util.spec_from_file_location("next_issue", os.path.join(HERE, "next-issue.py"))
next_issue = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(next_issue)

OWNER = "mkolehmainen"
PROJECT_NUMBER = 1
# Statuses this script owns. Anything else is someone's work in flight.
DERIVED = ("Backlog", "Ready")


def gh_graphql(query: str, **variables):
    """GraphQL call with bounded retries, erroring loudly on a GraphQL error."""
    cmd = ["api", "graphql", "-f", f"query={query}"]
    for key, value in variables.items():
        cmd += ["-F" if isinstance(value, int) else "-f", f"{key}={value}"]
    payload = json.loads(run_gh(cmd))
    if "errors" in payload:
        raise SystemExit("GraphQL error: " + json.dumps(payload["errors"], indent=2))
    return payload


BOARD_QUERY = """
query($owner:String!, $number:Int!) {
  user(login:$owner) { projectV2(number:$number) {
    id
    fields(first:30) { nodes {
      ... on ProjectV2SingleSelectField { id name options { id name } }
      ... on ProjectV2IterationField { id name
        configuration { duration iterations { id title startDate duration } } } } }
    items(first:100) { nodes {
      id
      content { ... on Issue { number state } }
      fieldValues(first:20) { nodes {
        ... on ProjectV2ItemFieldSingleSelectValue { name field { ... on ProjectV2FieldCommon { name } } }
        ... on ProjectV2ItemFieldIterationValue { title field { ... on ProjectV2FieldCommon { name } } } } } } } } }
}
"""


def current_iteration(iterations, today, default_duration=14):
    """The iteration containing `today`, or None if today falls outside them all.

    Iterations are contiguous and dated, so this is a plain range check rather
    than "the first one" -- picking the first would silently put work in a past
    iteration once one completes. `duration` is per-iteration when the API
    returns it and falls back to the field's configured duration, which is what
    a board with uniform-length iterations reports.
    """
    for it in iterations:
        start = datetime.date.fromisoformat(it["startDate"])
        length = it.get("duration") or default_duration
        if start <= today < start + datetime.timedelta(days=length):
            return it
    return None


def field_value(item, field_name):
    """The item's value for a named field, or None if unset."""
    for value in item["fieldValues"]["nodes"]:
        if not value:
            continue
        if (value.get("field") or {}).get("name") == field_name:
            return value.get("name") or value.get("title")
    return None


def build_plan(board, unblocked_unassigned, underway_numbers, today,
               umbrella_numbers=frozenset()):
    """Compute (plan, suggestions, skipped) without writing anything.

    plan: list of edits -- Iteration backfill only. Status is read-only since
          zipline#155, so no Status edit is ever planned, in either direction.
    suggestions: Backlog->Ready promotion candidates (open, unassigned,
          unblocked, Status still Backlog or unset). Printed for the operator,
          who owns `Ready`; never applied.
    skipped: items reported rather than acted on -- assigned items still in a
          derived status (drift), and Ready-but-blocked items (operator
          pre-authorization: auto-starts when the blockers close, so demoting
          it would erase the green-light).

    `unblocked_unassigned` is the set of issue numbers that are open,
    unassigned and have no open blocker -- the dependency-graph side of
    pickability, independent of board Status. `umbrella_numbers` are the
    tracking issues: containers, never pickable, so their Status is nobody's
    to suggest or report -- only their Iteration is backfilled.
    """
    fields = {f["name"]: f for f in board["fields"]["nodes"] if f}
    iteration_field = fields["Iteration"]
    configuration = iteration_field["configuration"]
    iteration = current_iteration(
        configuration["iterations"], today, configuration.get("duration") or 14
    )
    if iteration is None:
        raise SystemExit(
            f"no iteration contains {today}. Add one on the board before syncing."
        )

    plan, suggestions, skipped = [], [], []
    for item in board["items"]["nodes"]:
        content = item["content"] or {}
        number = content.get("number")
        if number is None or content.get("state") != "OPEN":
            continue

        status_now = field_value(item, "Status")
        if number in umbrella_numbers:
            pass  # container, not work: Status is not reported or suggested
        elif number in underway_numbers and status_now in DERIVED:
            skipped.append({"number": number, "status": status_now,
                            "why": "assigned but still in a derived status"})
        elif number in unblocked_unassigned and status_now in (None, "Backlog"):
            # Promotable, but Ready is operator-owned (zipline#155): suggest.
            suggestions.append({"number": number, "status": status_now})
        elif (status_now == "Ready" and number not in unblocked_unassigned
              and number not in underway_numbers):
            # Ready with an open blocker: the operator pre-authorized it and
            # it auto-starts when the blockers close. Report, never demote.
            skipped.append({"number": number, "status": status_now,
                            "why": "Ready but blocked -- pre-authorized, "
                                   "auto-starts when blockers close"})

        # Iteration is only ever filled in, never moved: reassigning an item
        # someone deliberately parked in a later iteration would be wrong.
        if field_value(item, "Iteration") is None:
            plan.append({"number": number, "field": "Iteration", "from": None,
                         "to": iteration["title"], "item_id": item["id"],
                         "field_id": iteration_field["id"], "value": iteration["id"],
                         "kind": "iteration"})
    plan.sort(key=lambda e: (e["number"], e["field"]))
    suggestions.sort(key=lambda s: s["number"])
    return plan, suggestions, skipped


# `gh api graphql -f` sends every variable as a string, so a `ProjectV2FieldValue`
# object variable is rejected ("provided invalid value"). Passing the id as a
# String and building the object literally in the query is what works.
ITERATION_MUTATION = """
mutation($project:ID!, $item:ID!, $field:ID!, $iteration:String!) {
  updateProjectV2ItemFieldValue(input:{
    projectId:$project, itemId:$item, fieldId:$field,
    value:{ iterationId:$iteration }
  }) { projectV2Item { id } }
}
"""


def apply_edit(project_id, edit):
    """Write one field value. Raises via gh_graphql on a GraphQL error.

    Only Iteration edits exist since zipline#155 -- build_plan never plans a
    Status edit, so there is deliberately no Status mutation here: `Ready` is
    operator-owned and this script must not be able to write it.
    """
    assert edit["kind"] == "iteration", edit
    gh_graphql(ITERATION_MUTATION, project=project_id, item=edit["item_id"],
               field=edit["field_id"], iteration=edit["value"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true",
                        help="write the plan (Iteration backfill only)")
    parser.add_argument("--json", action="store_true", dest="as_json")
    args = parser.parse_args()

    board = gh_graphql(BOARD_QUERY, owner=OWNER, number=PROJECT_NUMBER)
    board = board["data"]["user"]["projectV2"]
    # One fetch feeds both derivations: umbrellas and execution order come out
    # of the same sub-issue graph that `select` reads blockers from. Board
    # statuses come from the board fetch above rather than a second query.
    issues = next_issue.all_issues()
    order = next_issue.execution_order(issues)
    statuses = {}
    for item in board["items"]["nodes"]:
        number = (item["content"] or {}).get("number")
        if number is not None:
            statuses[number] = field_value(item, "Status")
    ready, pre_authorized, awaiting_ready, underway = next_issue.select(
        issues, order, statuses)
    plan, suggestions, skipped = build_plan(
        board,
        # The dependency-graph side of pickability, independent of Status:
        # `ready` is the Ready subset, `awaiting_ready` the rest.
        {r["number"] for r in ready} | {r["number"] for r in awaiting_ready},
        {u["number"] for u in underway},
        datetime.date.today(),
        next_issue.umbrellas(issues),
    )

    if args.as_json:
        print(json.dumps({"plan": plan, "suggestions": suggestions,
                          "skipped": skipped}, indent=2))
        return

    if not plan:
        print("Board already matches the dependency graph. Nothing to do.")
    for edit in plan:
        print(f"  #{edit['number']:<3} {edit['field']:<10} {edit['from']} -> {edit['to']}")
    for s in suggestions:
        print(f"  #{s['number']:<3} SUGGEST    {s['status'] or '(no status)'} -> Ready"
              f" -- unblocked and unassigned; flip it yourself to green-light pickup")
    for item in skipped:
        print(f"  #{item['number']:<3} SKIPPED    {item['status']} -- {item['why']}")

    if not args.apply:
        if plan:
            print(f"\n{len(plan)} edit(s) planned. Nothing written; re-run with --apply.")
        return
    for edit in plan:
        apply_edit(board["id"], edit)
    print(f"\nApplied {len(plan)} edit(s). Suggestions are never applied: "
          f"`Ready` is operator-owned (zipline#155).")


if __name__ == "__main__":
    main()
