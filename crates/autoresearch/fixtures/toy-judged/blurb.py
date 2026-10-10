"""The artefact a toy-judged study improves: one blurb per product, written to
$AUTORESEARCH_OUTPUT/<id>.txt. Cases come from $AUTORESEARCH_CASES (the study's snapshot)
or products.jsonl beside this file. System python3, no dependencies, no network.

The starting version is deliberately weak: it names only the first two features and pads the
paragraph past the rubric's 60 words.
"""
import json
import os
import sys


def blurb(name, features):
    first, second = features[0], features[1] if len(features) > 1 else features[0]
    return (
        f"Meet the {name}, the toy every family has been waiting for. It {first} and it also "
        f"{second}, which makes it a wonderful companion for rainy afternoons, sunny weekends, "
        f"long car journeys and lazy holiday mornings alike. Children adore it, parents trust it, "
        f"and grandparents keep asking where they can buy one too. Order yours today and discover "
        f"why the {name} is the gift that keeps on giving all year round."
    )


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    cases_path = os.environ.get("AUTORESEARCH_CASES") or os.path.join(here, "products.jsonl")
    out = os.environ.get("AUTORESEARCH_OUTPUT")
    if not out:
        print("blurb.py: AUTORESEARCH_OUTPUT is not set", file=sys.stderr)
        return 2
    os.makedirs(out, exist_ok=True)
    words = []
    with open(cases_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            case = json.loads(line)
            text = case["input"]
            name = text.split("\n", 1)[0].removeprefix("Product: ").strip()
            features = json.loads(text.split("Features: ", 1)[1])
            para = blurb(name, features)
            words.append(len(para.split()))
            with open(os.path.join(out, f"{case['id']}.txt"), "w", encoding="utf-8") as o:
                o.write(para + "\n")
    print(f"METRIC words={sum(words) / max(1, len(words)):.1f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
