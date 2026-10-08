"""CLI: read whitespace-separated ints from stdin, print them sorted one per line."""

import sys

from sorter import sort


def main():
    xs = [int(t) for t in sys.stdin.read().split()]
    sys.stdout.write("\n".join(str(x) for x in sort(xs)) + "\n")


if __name__ == "__main__":
    main()
