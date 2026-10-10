"""CLI: read one JSON document from stdin, print it re-serialised with sorted keys."""

import json
import sys

from jsonparse import parse


def main():
    sys.stdout.write(json.dumps(parse(sys.stdin.read()), sort_keys=True, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
