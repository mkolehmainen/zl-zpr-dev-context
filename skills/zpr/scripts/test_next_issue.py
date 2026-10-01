#!/usr/bin/env python3
"""Tests for next-issue.py's selection logic -- the part that decides what an
unattended agent picks up next. No network: `select` is pure, so the GraphQL
shape and the board-status map are supplied as fixtures.

Run: python3 test_next_issue.py
"""

import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

# The module has a hyphen in its name, so it cannot be imported by name.
spec = importlib.util.spec_from_file_location("next_issue", os.path.join(HERE, "next-issue.py"))
next_issue = importlib.util.module_from_spec(spec)
spec.loader.exec_module(next_issue)


def issue(number, blockers=(), assignees=(), label="core", state="OPEN", subs=()):
    """One GraphQL issue node.

    `blockers` is a list of (number, state) pairs; `subs` is a list of issue
    numbers, which is what makes this node an umbrella.
    """
    return {
        "number": number,
        "title": f"issue {number}",
        "url": f"https://github.com/mkolehmainen/zipline/issues/{number}",
        "state": state,
        "labels": {"nodes": [{"name": label}]},
        "assignees": {"nodes": [{"login": a} for a in assignees]},
        "blockedBy": {"nodes": [{"number": n, "state": s} for n, s in blockers]},
        "subIssues": {"nodes": [{"number": n} for n in subs]},
    }


def numbers(rows):
    return [r["number"] for r in rows]


def sel(issues, order, statuses=None):
    """Call select; by default every fixture issue is board-Ready, so the
    pre-existing dependency/assignment tests keep testing what they tested
    before the Ready gate existed."""
    if statuses is None:
        statuses = {i["number"]: "Ready" for i in issues}
    return next_issue.select(issues, order, statuses)


# --- dependency and assignment rules (pre-existing, all fixtures Ready) ------


def test_open_blocker_is_not_ready():
    ready, _, _, underway = sel([issue(5, blockers=[(4, "OPEN")])], {5: 0})
    assert numbers(ready) == [], ready
    assert numbers(underway) == [], underway


def test_closed_blockers_are_ready():
    ready, _, _, _ = sel([issue(5, blockers=[(4, "CLOSED"), (3, "CLOSED")])], {5: 0})
    assert numbers(ready) == [5], ready


def test_any_open_blocker_blocks():
    """A mix must block -- an `any` written as `all` would let this through."""
    ready, _, _, _ = sel([issue(5, blockers=[(4, "CLOSED"), (3, "OPEN")])], {5: 0})
    assert numbers(ready) == [], ready


def test_assigned_is_underway_not_ready():
    """The regression that matters: an issue being worked on must not be re-picked."""
    issues = [issue(17, assignees=["mkolehmainen"]), issue(2)]
    ready, _, _, underway = sel(issues, {17: 0, 2: 1})
    assert numbers(ready) == [2], ready
    assert numbers(underway) == [17], underway
    assert underway[0]["assignees"] == ["mkolehmainen"]


def test_umbrella_is_never_a_work_item():
    """An issue with sub-issues is a container for work, not a task.

    The regression this guards: with the umbrella hardcoded as `#1`, filing a
    second umbrella made it pickable, and an unattended agent was offered a
    whole feature as its next task.
    """
    issues = [issue(34, subs=[35, 36]), issue(35), issue(36)]
    ready, _, _, underway = sel(issues, {35: 0, 36: 1})
    assert numbers(ready) == [35, 36], ready
    assert numbers(underway) == [], underway


def test_closed_issue_is_neither_ready_nor_underway():
    """The query fetches every state so closed umbrellas can be read; closed
    issues must not come back out as work."""
    issues = [issue(2, state="CLOSED"), issue(5)]
    ready, _, _, underway = sel(issues, {2: 0, 5: 1})
    assert numbers(ready) == [5], ready
    assert numbers(underway) == [], underway


def test_execution_order_spans_umbrellas_oldest_first():
    """Two features in flight: the earlier umbrella's tasks come first, and
    within an umbrella its sub-issue order wins over issue number."""
    issues = [issue(34, subs=[36, 35]), issue(1, subs=[9, 4]),
              issue(4), issue(9), issue(35), issue(36)]
    order = next_issue.execution_order(issues)
    assert order == {9: 0, 4: 1, 36: 2, 35: 3}, order
    ready, _, _, _ = sel(issues, order)
    assert numbers(ready) == [9, 4, 36, 35], ready


def test_closed_umbrella_still_orders_its_open_children():
    """A finished feature can leave an open task behind -- it keeps its
    position rather than falling back to issue-number order."""
    issues = [issue(1, subs=[18, 9], state="CLOSED"), issue(18), issue(9)]
    order = next_issue.execution_order(issues)
    assert order == {18: 0, 9: 1}, order
    ready, _, _, _ = sel(issues, order)
    assert numbers(ready) == [18, 9], ready


def test_issue_on_no_umbrella_sorts_after_every_attached_one():
    issues = [issue(1, subs=[9]), issue(9), issue(3)]
    order = next_issue.execution_order(issues)
    ready, _, _, _ = sel(issues, order)
    assert numbers(ready) == [9, 3], ready


def test_execution_order_wins_over_issue_number():
    """Sub-issue position is the whole tiebreak, so a high number can be first."""
    ready, _, _, _ = sel([issue(2), issue(17), issue(8)], {17: 0, 2: 1, 8: 2})
    assert numbers(ready) == [17, 2, 8], ready


def test_unattached_issues_sort_after_attached_ones():
    """Anything not on the umbrella is not on the critical path -- it goes last."""
    ready, _, _, _ = sel([issue(18), issue(2)], {2: 5})
    assert numbers(ready) == [2, 18], ready


def test_underway_is_also_in_execution_order():
    issues = [issue(8, assignees=["a"]), issue(17, assignees=["b"])]
    _, _, _, underway = sel(issues, {17: 0, 8: 2})
    assert numbers(underway) == [17, 8], underway


# --- board Ready gate (zipline#155) ------------------------------------------
#
# pickable = open AND unassigned AND all blockers closed AND board Status == Ready.
# Ready is operator-owned. Per the operator's Q1 answer on zipline#155: an issue
# absent from the board, or on the board with no Status value, counts as Backlog
# and is NOT pickable.


def test_ready_status_unblocked_unassigned_is_pickable():
    ready, pre, awaiting, underway = next_issue.select([issue(5)], {5: 0}, {5: "Ready"})
    assert numbers(ready) == [5], ready
    assert numbers(pre) == [], pre
    assert numbers(awaiting) == [], awaiting
    assert numbers(underway) == [], underway


def test_backlog_status_is_not_pickable_but_reported_awaiting_ready():
    """Unblocked + unassigned + Backlog: the old rules would pick this up. The
    gate must hold it, and it must be *reported* rather than silently rot."""
    ready, pre, awaiting, underway = next_issue.select([issue(5)], {5: 0}, {5: "Backlog"})
    assert numbers(ready) == [], ready
    assert numbers(awaiting) == [5], awaiting
    assert numbers(pre) == [], pre
    assert numbers(underway) == [], underway


def test_absent_from_board_counts_as_backlog():
    """Operator Q1 on zipline#155: not on the board at all => Backlog => not
    pickable, reported under awaiting-ready."""
    ready, pre, awaiting, _ = next_issue.select([issue(5)], {5: 0}, {})
    assert numbers(ready) == [], ready
    assert numbers(awaiting) == [5], awaiting
    assert numbers(pre) == [], pre


def test_no_status_value_counts_as_backlog():
    """Operator Q1 on zipline#155: a board item with an unset Status counts as
    Backlog, same as absence."""
    ready, _, awaiting, _ = next_issue.select([issue(5)], {5: 0}, {5: None})
    assert numbers(ready) == [], ready
    assert numbers(awaiting) == [5], awaiting


def test_ready_but_blocked_is_pre_authorized_with_blocker_numbers():
    """Marking a blocked issue Ready is pre-authorization: held while any
    blocker is open, auto-starts when the last one closes. Reported with its
    open blocker numbers, never in `ready`."""
    ready, pre, awaiting, _ = next_issue.select(
        [issue(5, blockers=[(4, "OPEN"), (3, "CLOSED")])], {5: 0}, {5: "Ready"})
    assert numbers(ready) == [], ready
    assert numbers(pre) == [5], pre
    assert pre[0]["blocked_by"] == [4], pre
    assert numbers(awaiting) == [], awaiting


def test_backlog_and_blocked_appears_nowhere():
    """Blocked + Backlog is neither pickable, pre-authorized, nor awaiting:
    it needs both a closed blocker and an operator Ready."""
    ready, pre, awaiting, underway = next_issue.select(
        [issue(5, blockers=[(4, "OPEN")])], {5: 0}, {5: "Backlog"})
    for rows in (ready, pre, awaiting, underway):
        assert numbers(rows) == [], rows


def test_assigned_is_underway_regardless_of_board_status():
    """Assignment marks work in flight; the board gate applies to pickup, not
    to what is already claimed."""
    for status_map in ({17: "Ready"}, {17: "Backlog"}, {17: None}, {}):
        ready, pre, awaiting, underway = next_issue.select(
            [issue(17, assignees=["a"])], {17: 0}, status_map)
        assert numbers(underway) == [17], (status_map, underway)
        assert numbers(ready) == [], (status_map, ready)
        assert numbers(awaiting) == [], (status_map, awaiting)


# --- board items from other repositories (PR #47 review, P1) -----------------
#
# The board carries legacy `org-zpr/zpr-*` issues alongside the tracker's. The
# Status map is keyed by issue number, so a foreign item sharing a number with
# a `mkolehmainen/zipline` issue must be ignored -- otherwise an unrelated
# Ready item makes a Backlog tracker issue pickable (bypassing the operator
# gate), and an unrelated Backlog item suppresses an authorized one.


def board_page(*nodes, has_next=False, cursor=None):
    """One page of BOARD_QUERY results, as gh_graphql returns it."""
    return {"data": {"user": {"projectV2": {"items": {
        "nodes": list(nodes),
        "pageInfo": {"hasNextPage": has_next, "endCursor": cursor},
    }}}}}


def board_item(number, status, repo="mkolehmainen/zipline"):
    """One board item node: an issue with a repository and a Status value."""
    return {
        "content": {"number": number, "repository": {"nameWithOwner": repo}},
        "fieldValueByName": {"name": status} if status else None,
    }


def statuses_from(*pages):
    """Run board_statuses against canned pages instead of the network."""
    remaining = list(pages)
    original = next_issue.gh_graphql
    next_issue.gh_graphql = lambda *a, **k: remaining.pop(0)
    try:
        return next_issue.board_statuses()
    finally:
        next_issue.gh_graphql = original


def test_foreign_backlog_item_cannot_suppress_a_tracker_ready():
    """org-zpr/zpr-core#5 at Backlog arriving after tracker #5 at Ready must
    not overwrite it -- that would hold back an operator-authorized issue."""
    statuses = statuses_from(board_page(
        board_item(5, "Ready"),
        board_item(5, "Backlog", repo="org-zpr/zpr-core"),
    ))
    assert statuses.get(5) == "Ready", statuses


def test_foreign_ready_item_cannot_make_a_tracker_issue_pickable():
    """org-zpr/zpr-core#7 at Ready, tracker #7 not on the board: #7 must stay
    absent from the map (= Backlog = not pickable), or the operator gate is
    bypassed by an unrelated item."""
    statuses = statuses_from(board_page(
        board_item(7, "Ready", repo="org-zpr/zpr-core"),
    ))
    assert 7 not in statuses, statuses


def test_tracker_items_still_map_and_pagination_still_walks():
    statuses = statuses_from(
        board_page(board_item(5, "Ready"), has_next=True, cursor="c1"),
        board_page(board_item(6, "Backlog"),
                   board_item(6, "Ready", repo="org-zpr/zpr-visaservice")),
    )
    assert statuses == {5: "Ready", 6: "Backlog"}, statuses


def test_awaiting_ready_and_pre_authorized_sort_in_execution_order():
    ready, pre, awaiting, _ = next_issue.select(
        [issue(2), issue(17), issue(8, blockers=[(1, "OPEN")]),
         issue(9, blockers=[(1, "OPEN")])],
        {17: 0, 9: 1, 8: 2, 2: 3},
        {17: "Backlog", 2: "Backlog", 8: "Ready", 9: "Ready"})
    assert numbers(awaiting) == [17, 2], awaiting
    assert numbers(pre) == [9, 8], pre
    assert numbers(ready) == [], ready


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
    sys.exit(0)
