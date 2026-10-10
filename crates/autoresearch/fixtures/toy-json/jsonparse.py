"""The thing the study optimises: `parse(text)` turns a JSON document into Python
values (dict, list, str, int, float, bool, None).

Deliberately slow: a recursive-descent parser that re-slices the remaining text at
every token (`rest = rest[1:]`), so parsing is quadratic in the document length. A first
experiment has an obvious win (track an index, or use the standard library).
"""


def parse(text):
    value, rest = _value(text.lstrip())
    if rest.strip():
        raise ValueError("trailing data")
    return value


def _value(rest):
    c = rest[0]
    if c == "{":
        return _object(rest[1:])
    if c == "[":
        return _array(rest[1:])
    if c == '"':
        return _string(rest[1:])
    if rest.startswith("true"):
        return True, rest[4:]
    if rest.startswith("false"):
        return False, rest[5:]
    if rest.startswith("null"):
        return None, rest[4:]
    return _number(rest)


def _object(rest):
    out = {}
    rest = rest.lstrip()
    if rest[0] == "}":
        return out, rest[1:]
    while True:
        rest = rest.lstrip()
        key, rest = _string(rest[1:])
        rest = rest.lstrip()[1:]          # ':'
        value, rest = _value(rest.lstrip())
        out[key] = value
        rest = rest.lstrip()
        if rest[0] == ",":
            rest = rest[1:]
            continue
        return out, rest[1:]              # '}'


def _array(rest):
    out = []
    rest = rest.lstrip()
    if rest[0] == "]":
        return out, rest[1:]
    while True:
        value, rest = _value(rest.lstrip())
        out.append(value)
        rest = rest.lstrip()
        if rest[0] == ",":
            rest = rest[1:]
            continue
        return out, rest[1:]              # ']'


_ESC = {'"': '"', "\\": "\\", "/": "/", "b": "\b", "f": "\f", "n": "\n", "r": "\r", "t": "\t"}


def _string(rest):
    out = ""
    while True:
        c = rest[0]
        rest = rest[1:]
        if c == '"':
            return out, rest
        if c == "\\":
            e = rest[0]
            rest = rest[1:]
            if e == "u":
                out += chr(int(rest[:4], 16))
                rest = rest[4:]
            else:
                out += _ESC[e]
        else:
            out += c


def _number(rest):
    i = 0
    while i < len(rest) and rest[i] in "+-0123456789.eE":
        i += 1
    tok = rest[:i]
    if any(ch in tok for ch in ".eE"):
        return float(tok), rest[i:]
    return int(tok), rest[i:]
