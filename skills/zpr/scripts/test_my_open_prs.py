#!/usr/bin/env python3
"""Tests for my-open-prs.py's `needs_work` -- the predicate that decides whether
the retry loop re-wakes the automation. Pure, so no network is involved.

Run: python3 test_my_open_prs.py
"""

import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("my_open_prs", os.path.join(HERE, "my-open-prs.py"))
mop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mop)

BOT = "mkolehmainen"


def pr(**over):
    """A PR item with nothing outstanding; override one field per test."""
    item = {
        "draft": False,
        "unresolvedThreads": [],
        "checksState": "NONE",
        "mergeStateStatus": "CLEAN",
    }
    item.update(over)
    return item


def test_quiet_pr_needs_no_work():
    assert mop.needs_work(pr(), BOT) is False


def test_thread_from_someone_else_needs_work():
    assert mop.needs_work(pr(unresolvedThreads=[{"author": "reviewer"}]), BOT) is True


def test_own_unanswered_thread_does_not_loop():
    """The bot already replied last, so re-waking it would spin."""
    assert mop.needs_work(pr(unresolvedThreads=[{"author": BOT}]), BOT) is False


def test_failed_ci_does_not_wake_the_loop():
    """The regression: fork Actions is disabled, so a FAILURE rollup can only be
    historical and no push clears it -- keying on it looped forever."""
    assert mop.needs_work(pr(checksState="FAILURE"), BOT) is False
    assert mop.needs_work(pr(checksState="ERROR"), BOT) is False


def test_behind_or_dirty_needs_work():
    assert mop.needs_work(pr(mergeStateStatus="BEHIND"), BOT) is True
    assert mop.needs_work(pr(mergeStateStatus="DIRTY"), BOT) is True


def test_draft_is_never_work():
    """A draft is the author's to finish; nothing is waiting on the bot."""
    assert mop.needs_work(pr(draft=True, mergeStateStatus="DIRTY"), BOT) is False


# --- repository enumeration (the fork-blind-search regression) ---------------


WORKSPACE_SAMPLE = """
version: 1

workspace:
  name: zl_zpr

repositories:
  # Core ZPR components
  - name: zl-zpr-core
    url: git@github.com:mkolehmainen/zl-zpr-core.git
    default_branch: zipline

  - name: zl-zpr-common
    url: git@github.com:mkolehmainen/zl-zpr-common.git
    default_branch: zipline

documentation:
  root: docs
"""


def test_workspace_repos_are_parsed_in_file_order():
    assert mop.parse_workspace_repos(WORKSPACE_SAMPLE) == ["zl-zpr-core", "zl-zpr-common"]


def test_non_repository_name_keys_are_ignored():
    """`workspace: name: zl_zpr` and `documentation:` must not become repositories."""
    for junk in mop.parse_workspace_repos(WORKSPACE_SAMPLE):
        assert junk.startswith("zl-zpr-")


def test_empty_workspace_yields_nothing():
    assert mop.parse_workspace_repos("version: 1\n") == []


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok   {t.__name__}")
    print(f"\n{len(tests)} passed")
