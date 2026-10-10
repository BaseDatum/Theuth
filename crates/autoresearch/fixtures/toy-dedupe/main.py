"""CLI: read one token per line from stdin, print them deduplicated, first occurrence kept."""

import sys

from dedupe import dedupe


def main():
    xs = sys.stdin.read().split()
    sys.stdout.write("\n".join(dedupe(xs)) + "\n")


if __name__ == "__main__":
    main()
