"""CLI: first line `capacity`, then one key per line; prints the number of hits."""

import sys

from lru import replay


def main():
    lines = sys.stdin.read().split()
    sys.stdout.write(f"{replay(lines[1:], int(lines[0]))}\n")


if __name__ == "__main__":
    main()
