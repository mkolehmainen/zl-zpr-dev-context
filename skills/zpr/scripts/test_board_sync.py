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


# --- desired_status ----------------------------------------------------------


def test_unblocked_and_unassigned_becomes_ready():
    assert bs.desired_status(2, {2}, set(), "Backlog") == "Ready"


def test_blocked_becomes_backlog():
    assert bs.desired_status(4, {2}, set(), "Ready") == "Backlog"


def test_work_in_flight_status_is_never_touched():
    for status in ("In progress", "In review", "Done"):
        assert bs.desired_status(2, {2}, set(), status) is None, status


def test_assigned_item_is_left_for_a_human():
    """Assigned + still derived is drift; guessing In progress vs In review is wrong."""
    assert bs.desired_status(17, set(), {17}, "Backlog") is None


# --- build_plan -------------------------------------------------------------


def item(number, status=None, iteration=None, state="OPEN"):
    values = []
    if status:
        values.append({"name": status, "field": {"name": "Status"}})
    if iteration:
        values.append({"title": iteration, "field": {"name": "Iteration"}})
    return {
        "id": f"item-{number}",
        "content": {"number": number, "state": state},
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


def test_plan_sets_status_and_fills_empty_iteration():
    plan, skipped = build(board([item(2, status="Backlog")]), {2}, set())
    assert [(e["field"], e["to"]) for e in plan] == [
        ("Iteration", "Iteration 1"), ("Status", "Ready")]
    assert skipped == []


def test_existing_iteration_is_not_moved():
    """Someone parked this in Iteration 2 on purpose."""
    plan, _ = build(board([item(2, status="Ready", iteration="Iteration 2")]), {2}, set())
    assert plan == []


def test_closed_items_are_ignored():
    plan, _ = build(board([item(9, status="Backlog", state="CLOSED")]), set(), set())
    assert plan == []


def test_assigned_drift_is_reported_not_planned():
    plan, skipped = build(board([item(17, status="Backlog", iteration="Iteration 1")]),
                          set(), {17})
    assert plan == []
    assert [s["number"] for s in skipped] == [17]


def test_missing_iteration_for_today_is_a_hard_error():
    try:
        build(board([item(2, status="Backlog")]), {2}, set(), today=d("2027-01-01"))
    except SystemExit as exc:
        assert "no iteration contains" in str(exc)
    else:
        raise AssertionError("expected SystemExit")


def build(b, ready, underway, today=d("2026-09-03")):
    return bs.build_plan(b, ready, underway, today)


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
