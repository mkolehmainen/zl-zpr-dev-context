#!/usr/bin/env python3
"""Reconcile the `mk zl-zpr project` board with the dependency graph.

The board is documentation derived from two sources of truth -- GitHub native
`blockedBy` edges and the umbrella's sub-issue order -- so it can always be
rebuilt from them. This script does that rebuild for the two fields that are
mechanically derivable:

  Status     Backlog <-> Ready only. An unblocked, unassigned issue is Ready;
             a blocked one is Backlog. `In progress`, `In review` and `Done`
             are owned by whoever is doing the work, never by this script, so
             it will not overwrite them.
  Iteration  Every open item lands in the current iteration if it has none.
             Existing values are left alone.

An assigned issue sitting at Backlog or Ready is drift this script refuses to
guess at -- the workflow should have moved it to `In progress` -- so it is
reported and skipped rather than assigned a status by machine.

Reads state and prints a plan by default. Pass --apply to write.

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


def desired_status(number, ready_numbers, underway_numbers, current):
    """Status this item should have, or None to leave it alone.

    Returns None for anything this script does not own: an item already in a
    work-in-flight status, and an assigned item whose status is still derived
    (that is drift for a human to resolve, not for a machine to guess).
    """
    if current not in DERIVED:
        return None
    if number in underway_numbers:
        return None
    if number in ready_numbers:
        return "Ready"
    return "Backlog"


def field_value(item, field_name):
    """The item's value for a named field, or None if unset."""
    for value in item["fieldValues"]["nodes"]:
        if not value:
            continue
        if (value.get("field") or {}).get("name") == field_name:
            return value.get("name") or value.get("title")
    return None


def build_plan(board, ready_numbers, underway_numbers, today):
    """Compute (plan, skipped) without writing anything.

    plan: list of edits, each naming the issue, field and target value.
    skipped: assigned items still in a derived status -- reported, not guessed.
    """
    fields = {f["name"]: f for f in board["fields"]["nodes"] if f}
    status_field, iteration_field = fields["Status"], fields["Iteration"]
    options = {o["name"]: o["id"] for o in status_field["options"]}
    configuration = iteration_field["configuration"]
    iteration = current_iteration(
        configuration["iterations"], today, configuration.get("duration") or 14
    )
    if iteration is None:
        raise SystemExit(
            f"no iteration contains {today}. Add one on the board before syncing."
        )

    plan, skipped = [], []
    for item in board["items"]["nodes"]:
        content = item["content"] or {}
        number = content.get("number")
        if number is None or content.get("state") != "OPEN":
            continue

        status_now = field_value(item, "Status")
        if number in underway_numbers and status_now in DERIVED:
            skipped.append({"number": number, "status": status_now,
                            "why": "assigned but still in a derived status"})
        target = desired_status(number, ready_numbers, underway_numbers, status_now)
        if target and target != status_now:
            plan.append({"number": number, "field": "Status", "from": status_now,
                         "to": target, "item_id": item["id"],
                         "field_id": status_field["id"], "value": options[target],
                         "kind": "single_select"})

        # Iteration is only ever filled in, never moved: reassigning an item
        # someone deliberately parked in a later iteration would be wrong.
        if field_value(item, "Iteration") is None:
            plan.append({"number": number, "field": "Iteration", "from": None,
                         "to": iteration["title"], "item_id": item["id"],
                         "field_id": iteration_field["id"], "value": iteration["id"],
                         "kind": "iteration"})
    plan.sort(key=lambda e: (e["number"], e["field"]))
    return plan, skipped


# `gh api graphql -f` sends every variable as a string, so a `ProjectV2FieldValue`
# object variable is rejected ("provided invalid value"). Passing the id as a
# String and building the object literally in the query is what works.
STATUS_MUTATION = """
mutation($project:ID!, $item:ID!, $field:ID!, $option:String!) {
  updateProjectV2ItemFieldValue(input:{
    projectId:$project, itemId:$item, fieldId:$field,
    value:{ singleSelectOptionId:$option }
  }) { projectV2Item { id } }
}
"""

ITERATION_MUTATION = """
mutation($project:ID!, $item:ID!, $field:ID!, $iteration:String!) {
  updateProjectV2ItemFieldValue(input:{
    projectId:$project, itemId:$item, fieldId:$field,
    value:{ iterationId:$iteration }
  }) { projectV2Item { id } }
}
"""


def apply_edit(project_id, edit):
    """Write one field value. Raises via gh_graphql on a GraphQL error."""
    if edit["kind"] == "single_select":
        query, key = STATUS_MUTATION, "option"
    else:
        query, key = ITERATION_MUTATION, "iteration"
    gh_graphql(query, project=project_id, item=edit["item_id"],
               field=edit["field_id"], **{key: edit["value"]})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="write the plan")
    parser.add_argument("--json", action="store_true", dest="as_json")
    args = parser.parse_args()

    board = gh_graphql(BOARD_QUERY, owner=OWNER, number=PROJECT_NUMBER)
    board = board["data"]["user"]["projectV2"]
    order = next_issue.execution_order()
    ready, underway = next_issue.select(next_issue.open_issues(), order)
    plan, skipped = build_plan(
        board,
        {r["number"] for r in ready},
        {u["number"] for u in underway},
        datetime.date.today(),
    )

    if args.as_json:
        print(json.dumps({"plan": plan, "skipped": skipped}, indent=2))
        return

    if not plan:
        print("Board already matches the dependency graph. Nothing to do.")
    for edit in plan:
        print(f"  #{edit['number']:<3} {edit['field']:<10} {edit['from']} -> {edit['to']}")
    for item in skipped:
        print(f"  #{item['number']:<3} SKIPPED    {item['status']} -- {item['why']}")

    if not args.apply:
        if plan:
            print(f"\n{len(plan)} edit(s) planned. Nothing written; re-run with --apply.")
        return
    for edit in plan:
        apply_edit(board["id"], edit)
    print(f"\nApplied {len(plan)} edit(s).")


if __name__ == "__main__":
    main()
