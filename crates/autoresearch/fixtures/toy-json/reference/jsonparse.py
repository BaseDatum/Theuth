"""Reference solution (159 §14): the suite runner measures it at the outer study's
Start to set `best_known`. Off limits to the inner study."""

import json


def parse(text):
    return json.loads(text)
