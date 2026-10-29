#!/usr/bin/env python3
"""Tests for my-current-tasks.py. Pure: no network, no board.

Run: python3 test_my_current_tasks.py
"""

import datetime
import importlib.util
import os

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("my_current_tasks",
                                              os.path.join(HERE, "my-current-tasks.py"))
mct = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mct)


def node(number, assignees, status=None, iteration=None, repo="zl-zipline"):
    values = []
    if status:
        values.append({"__typename": "ProjectV2ItemFieldSingleSelectValue",
                       "name": status, "field": {"name": "Status"}})
    if iteration:
        values.append({"__typename": "ProjectV2ItemFieldIterationValue",
                       "title": iteration, "field": {"name": "Iteration"}})
    return {
        "fieldValues": {"nodes": values},
        "content": {"__typename": "Issue", "number": number, "title": f"t{number}",
                    "url": f"u{number}", "state": "OPEN",
                    "repository": {"name": repo},
                    "assignees": {"nodes": [{"login": a} for a in assignees]}},
    }


# --- the board moved to org project #6 "zl-zpr" (2026-10-29) ----------------


def test_queries_target_the_org_project():
    assert mct.OWNER == "org-zpr"
    assert mct.PROJECT_NUMBER == 6
    for q in (mct.QUERY, mct.ITER_CFG_QUERY):
        assert "organization(login:$owner)" in q
        assert "user(login:" not in q


def test_project_reads_the_organization_root():
    payload = {"data": {"organization": {"projectV2": {"title": "zl-zpr"}}}}
    assert mct.project_of(payload) == {"title": "zl-zpr"}


def test_board_without_iteration_field_has_no_iteration_config():
    """Project #6 has no Iteration field. GitHub errors on `field(name:
    "Iteration")` for such a board (it does not return null), so the config is
    read from the `fields` list: no Iteration node there means no config."""
    payload = {"data": {"organization": {"projectV2": {"fields": {"nodes": [
        {}, {"name": "Status"}]}}}}}
    assert mct.iteration_config(payload) is None


def test_iteration_config_query_does_not_name_a_field_that_may_not_exist():
    assert 'field(name:"Iteration")' not in mct.ITER_CFG_QUERY
    assert "fields(first:" in mct.ITER_CFG_QUERY


def test_iteration_config_lists_iterations_when_present():
    its = [{"title": "Iteration 1", "startDate": "2026-09-03", "duration": 14}]
    payload = {"data": {"organization": {"projectV2": {"fields": {"nodes": [
        {}, {"name": "Iteration", "configuration": {"iterations": its}}]}}}}}
    assert mct.iteration_config(payload) == its


def test_current_iteration_window():
    its = [{"title": "Iteration 1", "startDate": "2026-09-03", "duration": 14}]
    assert mct.current_iteration(its, datetime.date(2026, 9, 16))[0] == "Iteration 1"
    assert mct.current_iteration(its, datetime.date(2026, 9, 17))[0] is None


def test_select_without_iteration_field_lists_every_assigned_item():
    nodes = [node(1, ["ZprBot2"]), node(2, ["someone"]), node(3, ["ZprBot2"], iteration="I9")]
    got = mct.select_matches(nodes, "ZprBot2", iteration=None)
    assert [m["number"] for m in got] == [1, 3]


def test_select_with_iteration_keeps_only_the_current_one():
    nodes = [node(1, ["ZprBot2"], iteration="I1"), node(2, ["ZprBot2"], iteration="I2"),
             node(3, ["ZprBot2"])]
    got = mct.select_matches(nodes, "ZprBot2", iteration="I1")
    assert [m["number"] for m in got] == [1]


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
