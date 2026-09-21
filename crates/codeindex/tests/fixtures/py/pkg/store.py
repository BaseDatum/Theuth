import os

DEFAULT_PATH = "index.db"


class Store:
    """A store."""

    def __init__(self, path=DEFAULT_PATH):
        self.path = path

    def get(self, key):
        return os.environ.get(key)

    def _private(self):
        return None


def apply(f, x):
    # `f` is a parameter: filtered as a local.
    return f(x) + helper(x)


def helper(x):
    return x + 1


def run():
    s = Store()
    return apply(lambda v: v * 2, 3) + len(s.path)
