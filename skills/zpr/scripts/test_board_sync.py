#!/usr/bin/env python3
"""Tests for board-sync.py's planning logic. Pure: no network, no board.

Run: python3 test_board_sync.py
"""

import datetime
import importlib.util
import os

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("board_sync", os.path.join(HERE, "board-sync.py"))
bs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bs)

ITERATIONS = [
    {"id": "i1", "title": "Iteration 1", "startDate": "2026-09-03", "duration": 14},
    {"id": "i2", "title": "Iteration 2", "startDate": "2026-09-17", "duration": 14},
]


def d(s):
    return datetime.date.fromisoformat(s)


# --- current_iteration -------------------------------------------------------


def test_first_day_is_inside_the_iteration():
    assert bs.current_iteration(ITERATIONS, d("2026-09-03"))["id"] == "i1"


def test_last_day_is_inside_the_iteration():
    assert bs.current_iteration(ITERATIONS, d("2026-09-16"))["id"] == "i1"


def test_day_after_rolls_to_the_next_iteration():
    """Off-by-one at the boundary would park work in a finished iteration."""
    assert bs.current_iteration(ITERATIONS, d("2026-09-17"))["id"] == "i2"


def test_duration_falls_back_to_the_field_configuration():
    """The live API omits per-iteration duration unless asked; don't crash on it."""
    bare = [{"id": "i1", "title": "Iteration 1", "startDate": "2026-09-03"}]
    assert bs.current_iteration(bare, d("2026-09-10"), 14)["id"] == "i1"
    assert bs.current_iteration(bare, d("2026-09-18"), 14) is None


def test_before_and_after_all_iterations_is_none():
    assert bs.current_iteration(ITERATIONS, d("2026-09-01")) is None
    assert bs.current_iteration(ITERATIONS, d("2026-12-01")) is None


# --- build_plan --------------------------------------------------------------


def item(number, status=None, iteration=None, state="OPEN",
         repo="mkolehmainen/zipline"):
    values = []
    if status:
        values.append({"name": status, "field": {"name": "Status"}})
    if iteration:
        values.append({"title": iteration, "field": {"name": "Iteration"}})
    return {
        "id": f"item-{repo}-{number}",
        "content": {"number": number, "state": state,
                    "repository": {"nameWithOwner": repo}},
        "fieldValues": {"nodes": values},
    }


def board(items):
    return {
        "id": "proj",
        "fields": {"nodes": [
            {"id": "sf", "name": "Status", "options": [
                {"id": "backlog-id", "name": "Backlog"},
                {"id": "ready-id", "name": "Ready"},
            ]},
            {"id": "itf", "name": "Iteration",
             "configuration": {"duration": 14, "iterations": ITERATIONS}},
        ]},
        "items": {"nodes": items},
    }


# --- the Ready gate (zipline#155): promotion is suggest-only -----------------


def test_promotion_is_suggested_never_planned():
    """An unblocked, unassigned Backlog item used to get a Backlog->Ready edit.
    Ready is operator-owned now: the item shows up as a *suggestion* and the
    plan carries no Status edit -- so even --apply cannot promote it."""
    plan, suggestions, skipped = build(
        board([item(2, status="Backlog", iteration="Iteration 1")]), {2}, set())
    assert plan == [], plan
    assert [s["number"] for s in suggestions] == [2], suggestions
    assert skipped == []


def test_plan_never_contains_a_status_edit():
    """The whole Status field is read-only to this script now, in both
    directions -- promotion AND demotion are reporting duties."""
    plan, _, _ = build(board([
        item(2, status="Backlog", iteration="Iteration 1"),   # promotable
        item(4, status="Ready", iteration="Iteration 1"),     # blocked, Ready
        item(6, iteration="Iteration 1"),                     # no status at all
    ]), {2, 6}, set())
    assert [e for e in plan if e["field"] == "Status"] == [], plan


def test_ready_but_blocked_is_reported_not_demoted():
    """A blocked issue sitting at Ready is pre-authorization (zipline#155): it
    auto-starts when its blockers close. Demoting it would erase the
    operator's green-light, so it is reported and left alone."""
    plan, suggestions, skipped = build(
        board([item(4, status="Ready", iteration="Iteration 1")]), set(), set())
    assert plan == [], plan
    assert suggestions == [], suggestions
    assert [s["number"] for s in skipped] == [4], skipped
    assert "pre-authorized" in skipped[0]["why"], skipped


def test_work_in_flight_status_is_never_touched_or_suggested():
    for status in ("In progress", "In review", "Done"):
        plan, suggestions, _ = build(
            board([item(2, status=status, iteration="Iteration 1")]), {2}, set())
        assert plan == [], (status, plan)
        assert suggestions == [], (status, suggestions)


def test_umbrella_gets_no_status_suggestion_or_report():
    """An umbrella is a container, not work: it is never pickable, so its
    Status is not this script's business in either direction -- no promotion
    suggestion, no pre-authorized report. Iteration backfill still applies."""
    for status in ("Backlog", "Ready"):
        plan, suggestions, skipped = build(
            board([item(140, status=status)]), set(), set(), umbrellas={140})
        assert [(e["field"], e["number"]) for e in plan] == [("Iteration", 140)], plan
        assert suggestions == [], (status, suggestions)
        assert skipped == [], (status, skipped)


# --- board items from other repositories (PR #47 review, P1) -----------------
#
# The board carries legacy `org-zpr/zpr-*` issues alongside the tracker's.
# build_plan keys its decisions by bare issue number, so a foreign item sharing
# a number with a tracker issue would be judged by the tracker issue's
# dependency state -- and edited or suggested as if it were the tracker issue.
# Foreign items are not this script's to touch: skip them entirely.


def test_foreign_item_is_not_suggested_or_edited():
    """org-zpr/zpr-core#2 shares a number with an unblocked tracker issue.
    It must get no Ready suggestion and no Iteration backfill -- it is not
    the tracker issue #2, whatever its number says."""
    plan, suggestions, skipped = build(
        board([item(2, status="Backlog", repo="org-zpr/zpr-core")]), {2}, set())
    assert plan == [], plan
    assert suggestions == [], suggestions
    assert skipped == [], skipped


def test_foreign_item_does_not_shadow_the_tracker_item():
    """Both #2s on the board: only the tracker one is planned or suggested."""
    plan, suggestions, _ = build(board([
        item(2, status="Backlog", repo="org-zpr/zpr-core"),
        item(2, status="Backlog"),
    ]), {2}, set())
    assert [e["item_id"] for e in plan] == ["item-mkolehmainen/zipline-2"], plan
    assert [s["number"] for s in suggestions] == [2], suggestions


# --- iteration backfill (unchanged duty) -------------------------------------


def test_plan_fills_empty_iteration():
    plan, suggestions, skipped = build(board([item(2, status="Backlog")]), {2}, set())
    assert [(e["field"], e["to"]) for e in plan] == [("Iteration", "Iteration 1")], plan
    assert [s["number"] for s in suggestions] == [2], suggestions
    assert skipped == []


def test_existing_iteration_is_not_moved():
    """Someone parked this in Iteration 2 on purpose."""
    plan, _, _ = build(board([item(2, status="Ready", iteration="Iteration 2")]), {2}, set())
    assert plan == []


def test_closed_items_are_ignored():
    plan, suggestions, _ = build(
        board([item(9, status="Backlog", state="CLOSED")]), set(), set())
    assert plan == []
    assert suggestions == []


def test_assigned_drift_is_reported_not_planned():
    plan, suggestions, skipped = build(
        board([item(17, status="Backlog", iteration="Iteration 1")]), set(), {17})
    assert plan == []
    assert suggestions == []
    assert [s["number"] for s in skipped] == [17]


def test_missing_iteration_for_today_is_a_hard_error():
    try:
        build(board([item(2, status="Backlog")]), {2}, set(), today=d("2027-01-01"))
    except SystemExit as exc:
        assert "no iteration contains" in str(exc)
    else:
        raise AssertionError("expected SystemExit")


def build(b, ready, underway, today=d("2026-09-03"), umbrellas=frozenset()):
    return bs.build_plan(b, ready, underway, today, umbrellas)


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
