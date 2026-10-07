#!/usr/bin/env python3
"""Tests for next-issue.py's selection logic -- the part that decides what an
unattended agent picks up next. No network: `select` is pure, so the GraphQL
shape and the board-status map are supplied as fixtures.

Run: python3 test_next_issue.py
"""

import importlib.util
import json
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


# --- second source: org-zpr/zipline via org project #5 (2026-10-07) ----------
#
# pickable(org) = open AND all blockers closed AND Status == Ready
#                 AND Iteration == current
#                 AND (unassigned OR assigned solely to the bot)
#
# Only org-zpr/zipline issues are candidates; legacy org-zpr/zpr-* items on
# the same project are never work for this agent. Assignment to the bot is an
# opt-in, not an underway marker (underway = Status In progress / In review);
# assignment to anyone else makes the issue theirs, skipped entirely.

import datetime  # noqa: E402

TODAY = datetime.date(2026, 10, 7)
CURRENT = {"startDate": "2026-09-29", "duration": 14, "title": "Iteration 3"}
NEXT_IT = {"startDate": "2026-10-13", "duration": 14, "title": "Iteration 4"}


def org_item(number, status="Ready", iteration=CURRENT, assignees=(),
             blockers=(), state="OPEN", subs=(), repo="org-zpr/zipline"):
    """One org project #5 item node, as ORG_BOARD_QUERY returns it."""
    return {
        "content": {
            "number": number,
            "title": f"org issue {number}",
            "url": f"https://github.com/org-zpr/zipline/issues/{number}",
            "state": state,
            "repository": {"nameWithOwner": repo},
            "labels": {"nodes": []},
            "assignees": {"nodes": [{"login": a} for a in assignees]},
            "blockedBy": {"nodes": [{"number": n, "state": s} for n, s in blockers]},
            "subIssues": {"nodes": [{"number": n} for n in subs]},
        },
        "status": {"name": status} if status else None,
        "iteration": iteration,
    }


def org_sel(items):
    return next_issue.select_org(items, TODAY, bot="ZprBot1")


def test_org_ready_current_unassigned_is_pickable():
    ready, pre, awaiting, underway, expired = org_sel([org_item(29)])
    assert numbers(ready) == [29], ready
    assert ready[0]["tracker"] == "org-zpr/zipline"
    for rows in (pre, awaiting, underway):
        assert numbers(rows) == [], rows


def test_org_bot_assignment_is_opt_in_not_underway():
    """Explicitly assigning the bot must not suppress pickup -- that is how
    the operator hands the bot a specific issue."""
    ready, _, _, underway, _ = org_sel([org_item(29, assignees=["ZprBot1"])])
    assert numbers(ready) == [29], ready
    assert numbers(underway) == [], underway


def test_org_issue_assigned_to_someone_else_is_theirs():
    """Assigned to another login (or bot + another): never picked, never
    reported -- taking it over would violate the assignment rule."""
    for assignees in (["mkolehmainen"], ["ZprBot1", "mkolehmainen"]):
        ready, pre, awaiting, underway, expired = org_sel(
            [org_item(29, assignees=assignees)])
        for rows in (ready, pre, awaiting, underway, expired):
            assert numbers(rows) == [], (assignees, rows)


def test_org_in_progress_status_is_underway():
    """Status In progress / In review is the org underway marker -- pickup
    sets it at claim time, and a later tick must poll, not re-pick."""
    for status in ("In progress", "In review"):
        ready, _, _, underway, _ = org_sel(
            [org_item(29, status=status, assignees=["ZprBot1"])])
        assert numbers(underway) == [29], (status, underway)
        assert numbers(ready) == [], (status, ready)


def test_org_legacy_zpr_repo_item_is_never_a_candidate():
    """The org project carries legacy org-zpr/zpr-* issues; they are never
    work for this agent, whatever their Status says."""
    ready, pre, awaiting, underway, expired = org_sel(
        [org_item(55, repo="org-zpr/zpr-visaservice")])
    for rows in (ready, pre, awaiting, underway, expired):
        assert numbers(rows) == [], rows


def test_org_future_iteration_is_pre_authorized_not_ready():
    """Ready in a not-yet-current iteration is scheduled-ahead: held, reported
    under pre-authorized, auto-starts when the iteration begins."""
    ready, pre, _, _, _ = org_sel([org_item(29, iteration=NEXT_IT)])
    assert numbers(ready) == [], ready
    assert numbers(pre) == [29], pre
    assert "Iteration 4" in pre[0]["held_by"], pre


def test_org_no_iteration_is_held():
    """Ready with no Iteration value is not scheduled -> not pickable."""
    ready, pre, _, _, _ = org_sel([org_item(29, iteration=None)])
    assert numbers(ready) == [], ready
    assert numbers(pre) == [29], pre


def test_org_open_blocker_holds_a_ready_item():
    ready, pre, _, _, _ = org_sel([org_item(29, blockers=[(28, "OPEN")])])
    assert numbers(ready) == [], ready
    assert numbers(pre) == [29], pre
    assert pre[0]["blocked_by"] == [28], pre


def test_org_backlog_unblocked_is_awaiting_ready():
    ready, _, awaiting, _, _ = org_sel([org_item(29, status="Backlog")])
    assert numbers(ready) == [], ready
    assert numbers(awaiting) == [29], awaiting


def test_org_absent_status_counts_as_backlog():
    ready, _, awaiting, _, _ = org_sel([org_item(29, status=None)])
    assert numbers(ready) == [], ready
    assert numbers(awaiting) == [29], awaiting


def test_org_closed_and_umbrella_items_are_skipped():
    ready, pre, awaiting, underway, expired = org_sel([
        org_item(10, state="CLOSED"),
        org_item(11, subs=[12, 13]),
    ])
    for rows in (ready, pre, awaiting, underway, expired):
        assert numbers(rows) == [], rows


def test_org_rows_sort_after_every_tracker_row():
    """Tracker precedence: an org row's position must exceed any tracker
    row's, attached to an umbrella or not."""
    tracker_ready, _, _, _ = sel([issue(18)], {})  # unattached: 10_000 + n
    org_ready, _, _, _, _ = org_sel([org_item(1)])
    assert org_ready[0]["position"] > tracker_ready[0]["position"]


def test_org_tiebreak_is_lowest_issue_number():
    ready, _, _, _, _ = org_sel([org_item(40), org_item(29)])
    assert numbers(ready) == [29, 40], ready


def test_iteration_is_current_boundaries():
    f = next_issue.iteration_is_current
    it = {"startDate": "2026-09-29", "duration": 14}
    assert f(it, datetime.date(2026, 9, 29))       # first day: current
    assert f(it, datetime.date(2026, 10, 12))      # last day: current
    assert not f(it, datetime.date(2026, 10, 13))  # day after: next iteration
    assert not f(it, datetime.date(2026, 9, 28))   # day before
    assert not f(None, TODAY)                      # no value at all
    assert not f({}, TODAY)                        # value without startDate


# --- expired iterations (PR #61 review, Codex P2) ---------------------------
#
# A Ready item whose Iteration has already ENDED can never auto-start: unlike a
# scheduled-ahead item, no future tick makes its iteration current. Reporting
# it under pre-authorized ("auto-starts when ...") would hide an item that
# needs rescheduling, so it gets its own section and is never pickable.

PAST_IT = {"startDate": "2026-09-15", "duration": 14, "title": "Iteration 2"}


def test_org_ended_iteration_is_expired_not_pre_authorized():
    ready, pre, awaiting, underway, expired = org_sel(
        [org_item(29, iteration=PAST_IT)])
    assert numbers(expired) == [29], expired
    assert "Iteration 2" in expired[0]["iteration"], expired
    for rows in (ready, pre, awaiting, underway):
        assert numbers(rows) == [], rows


def test_org_ended_iteration_with_open_blocker_is_still_expired():
    """A blocker closing later would not make it pickable either -- the
    iteration is the hold that needs a human, so it is reported as expired,
    with its open blockers carried along for the reschedule decision."""
    ready, pre, _, _, expired = org_sel(
        [org_item(29, iteration=PAST_IT, blockers=[(28, "OPEN")])])
    assert numbers(ready) == [] and numbers(pre) == [], (ready, pre)
    assert numbers(expired) == [29], expired
    assert expired[0]["blocked_by"] == [28], expired


def test_org_future_and_current_iterations_are_not_expired():
    _, _, _, _, expired = org_sel(
        [org_item(29, iteration=NEXT_IT), org_item(30, iteration=CURRENT)])
    assert numbers(expired) == [], expired


def test_org_expired_only_applies_to_ready_items():
    """Backlog / In progress items in an ended iteration keep their existing
    classification: the expired section is about Ready items that the gate
    would otherwise hold forever without saying why."""
    _, _, awaiting, underway, expired = org_sel([
        org_item(29, status="Backlog", iteration=PAST_IT),
        org_item(30, status="In progress", iteration=PAST_IT),
    ])
    assert numbers(expired) == [], expired
    assert numbers(awaiting) == [29], awaiting
    assert numbers(underway) == [30], underway


def test_iteration_has_ended_boundaries():
    f = next_issue.iteration_has_ended
    it = {"startDate": "2026-09-29", "duration": 14}
    assert not f(it, datetime.date(2026, 10, 12))  # last day: still current
    assert f(it, datetime.date(2026, 10, 13))      # day after: ended
    assert not f(it, datetime.date(2026, 9, 28))   # before start: future
    assert not f(None, TODAY)                      # unscheduled is not ended
    assert not f({}, TODAY)


def test_main_reports_expired_iteration_in_json_and_text():
    """End to end through main(): --json carries an `expired_iteration` key and
    the text report a reschedule section; neither offers the item as NEXT."""
    import contextlib
    import io
    long_ago = {"startDate": "2020-01-06", "duration": 14, "title": "Iteration 0"}
    saved = (next_issue.all_issues, next_issue.board_statuses,
             next_issue.org_items, list(sys.argv))
    next_issue.all_issues = lambda: []
    next_issue.board_statuses = lambda: {}
    next_issue.org_items = lambda: [org_item(29, iteration=long_ago)]
    try:
        buf = io.StringIO()
        sys.argv[:] = ["next-issue.py", "--json"]
        with contextlib.redirect_stdout(buf):
            next_issue.main()
        report = json.loads(buf.getvalue())
        assert report["next"] is None, report
        assert numbers(report["expired_iteration"]) == [29], report
        assert numbers(report["pre_authorized"]) == [], report

        buf = io.StringIO()
        sys.argv[:] = ["next-issue.py"]
        with contextlib.redirect_stdout(buf):
            next_issue.main()
        text = buf.getvalue()
        assert "iteration ended" in text and "reschedule" in text, text
        assert "org-zpr/zipline#29" in text, text
        assert "Iteration 0" in text, text
    finally:
        (next_issue.all_issues, next_issue.board_statuses,
         next_issue.org_items, argv) = saved
        sys.argv[:] = argv

if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
    sys.exit(0)
