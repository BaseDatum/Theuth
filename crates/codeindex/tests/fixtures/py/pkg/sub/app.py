"""Fixture: relative and absolute imports into the package."""

import os
from ..store import Store, apply
from pkg import store
from . import helpers


def main():
    s = Store(os.getcwd())
    store.run()
    return apply(lambda v: v, helpers.twice(1)) + len(s.path)
