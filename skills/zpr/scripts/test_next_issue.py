#!/usr/bin/env python3
"""Tests for next-issue.py's selection logic -- the part that decides what an
unattended agent picks up next. No network: `select` is pure, so the GraphQL
shape is supplied as fixtures.

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


def issue(number, blockers=(), assignees=(), label="core"):
    """One GraphQL issue node. `blockers` is a list of (number, state) pairs."""
    return {
        "number": number,
        "title": f"issue {number}",
        "url": f"https://github.com/mkolehmainen/zipline/issues/{number}",
        "labels": {"nodes": [{"name": label}]},
        "assignees": {"nodes": [{"login": a} for a in assignees]},
        "blockedBy": {"nodes": [{"number": n, "state": s} for n, s in blockers]},
    }


def numbers(rows):
    return [r["number"] for r in rows]


def test_open_blocker_is_not_ready():
    ready, underway = next_issue.select([issue(5, blockers=[(4, "OPEN")])], {5: 0})
    assert numbers(ready) == [], ready
    assert numbers(underway) == [], underway


def test_closed_blockers_are_ready():
    ready, _ = next_issue.select([issue(5, blockers=[(4, "CLOSED"), (3, "CLOSED")])], {5: 0})
    assert numbers(ready) == [5], ready


def test_any_open_blocker_blocks():
    """A mix must block -- an `any` written as `all` would let this through."""
    ready, _ = next_issue.select([issue(5, blockers=[(4, "CLOSED"), (3, "OPEN")])], {5: 0})
    assert numbers(ready) == [], ready


def test_assigned_is_underway_not_ready():
    """The regression that matters: an issue being worked on must not be re-picked."""
    issues = [issue(17, assignees=["mkolehmainen"]), issue(2)]
    ready, underway = next_issue.select(issues, {17: 0, 2: 1})
    assert numbers(ready) == [2], ready
    assert numbers(underway) == [17], underway
    assert underway[0]["assignees"] == ["mkolehmainen"]


def test_umbrella_is_never_a_work_item():
    ready, underway = next_issue.select([issue(next_issue.UMBRELLA)], {})
    assert numbers(ready) == [], ready
    assert numbers(underway) == [], underway


def test_execution_order_wins_over_issue_number():
    """Sub-issue position is the whole tiebreak, so a high number can be first."""
    ready, _ = next_issue.select([issue(2), issue(17), issue(8)], {17: 0, 2: 1, 8: 2})
    assert numbers(ready) == [17, 2, 8], ready


def test_unattached_issues_sort_after_attached_ones():
    """Anything not on the umbrella is not on the critical path -- it goes last."""
    ready, _ = next_issue.select([issue(18), issue(2)], {2: 5})
    assert numbers(ready) == [2, 18], ready


def test_underway_is_also_in_execution_order():
    issues = [issue(8, assignees=["a"]), issue(17, assignees=["b"])]
    _, underway = next_issue.select(issues, {17: 0, 8: 2})
    assert numbers(underway) == [17, 8], underway


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
    sys.exit(0)
