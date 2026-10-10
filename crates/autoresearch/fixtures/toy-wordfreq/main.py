"""CLI: read text from stdin, print the 50 most frequent words as `word count`, one per line."""

import sys

from wordfreq import top


def main():
    words = sys.stdin.read().split()
    sys.stdout.write("".join(f"{w} {c}\n" for w, c in top(words, 50)))


if __name__ == "__main__":
    main()
