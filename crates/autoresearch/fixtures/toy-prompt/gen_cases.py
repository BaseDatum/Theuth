#!/usr/bin/env python3
"""toy-prompt (159 §14): 90 "normalise this date to ISO 8601" cases, fixed seed, exact checks.

Nine input classes × ten dates. Each class has one rule a prompt must state to get it right
(the `tags` entry names the class); the ideal is always `YYYY-MM-DD`. Regenerate with
`python3 gen_cases.py > cases.jsonl` — the output is byte-stable.
"""
import datetime as dt
import json
import random

SEED = 159
MONTHS = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
]
ORDINALS = {
    1: "first", 2: "second", 3: "third", 4: "fourth", 5: "fifth", 6: "sixth", 7: "seventh",
    8: "eighth", 9: "ninth", 10: "tenth", 11: "eleventh", 12: "twelfth", 13: "thirteenth",
    14: "fourteenth", 15: "fifteenth", 16: "sixteenth", 17: "seventeenth", 18: "eighteenth",
    19: "nineteenth", 20: "twentieth", 21: "twenty-first", 22: "twenty-second",
    23: "twenty-third", 24: "twenty-fourth", 25: "twenty-fifth", 26: "twenty-sixth",
    27: "twenty-seventh", 28: "twenty-eighth",
}


def suffix(d):
    if 11 <= d % 100 <= 13:
        return "th"
    return {1: "st", 2: "nd", 3: "rd"}.get(d % 10, "th")


CLASSES = {
    # tag: (renderer, the rule the prompt must state)
    "long": (lambda d: f"{d.day} {MONTHS[d.month - 1]} {d.year}", "day month year in words"),
    "us_long": (lambda d: f"{MONTHS[d.month - 1]} {d.day}, {d.year}", "month day, year"),
    "us_slash": (lambda d: f"{d.month:02d}/{d.day:02d}/{d.year}", "slashes are MM/DD/YYYY (US)"),
    "eu_dots": (lambda d: f"{d.day}.{d.month}.{d.year}", "dots are DD.MM.YYYY (European)"),
    "ymd_slash": (lambda d: f"{d.year}/{d.month}/{d.day}", "a leading year is YYYY/M/D"),
    "short_year": (lambda d: f"{MONTHS[d.month - 1][:3]} {d.day} {d.year % 100:02d}", "a two-digit year is 20YY"),
    "ordinal_words": (lambda d: f"the {ORDINALS[d.day]} of {MONTHS[d.month - 1]} {d.year}", "ordinal words name the day"),
    "ordinal_suffix": (lambda d: f"{d.day}{suffix(d.day)} {MONTHS[d.month - 1][:3]} {d.year}", "a day with st/nd/rd/th"),
    "compact": (lambda d: f"{d.year}{d.month:02d}{d.day:02d}", "eight digits are YYYYMMDD"),
}


def main():
    rng = random.Random(SEED)
    n = 0
    for tag, (render, _rule) in CLASSES.items():
        for _ in range(10):
            d = dt.date(2000 + rng.randrange(0, 26), rng.randrange(1, 13), rng.randrange(1, 29))
            n += 1
            case = {
                "id": f"{tag}-{n:02d}",
                "input": f"Normalise this date to ISO 8601: {render(d)}",
                "ideal": d.isoformat(),
                "check": {"kind": "exact", "value": d.isoformat()},
                "tags": [tag],
            }
            print(json.dumps(case, ensure_ascii=False))


if __name__ == "__main__":
    main()
