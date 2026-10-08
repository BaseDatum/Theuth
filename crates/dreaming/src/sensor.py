# theuth-sensor: dreaming 2
# arg: window optional 01:00-06:00 — local window the night may start in (from dreaming.window)
# arg: idle optional 20 — minutes since the last root turn (dreaming.idle_minutes)
# arg: heavy_concepts optional 300 — new concepts that earn a second window (dreaming.heavy_day.concepts)
# arg: heavy_sessions optional 40 — sessions that earn a second window (dreaming.heavy_day.sessions)
# arg: engine optional <id> — refuse when signals.json was written by another engine (mesh)
"""Dreaming's gate (design 136 §4 phase 0): fires when the Mac is idle inside the night window.

when: dreaming.py [window=01:00-06:00] [idle=20] [heavy_concepts=300] [heavy_sessions=40] [engine=<id>]

Reads `dreaming/signals.json` under the routine's cwd (the data dir) — written by
the dreaming plugin from `TurnFinished`, knowledge and power events — and the
clock. Every rule must hold; the payload names the first that does not. The
same table lives in `crates/dreaming/src/sensor_rules.rs` (unit-tested); keep
the two in lockstep. State is unused; `next` is None (the gate keeps `every`).
"""
import datetime
import json

BATTERY_MIN_PCT = 40


def _hhmm(s: str):
    parts = s.strip().split(":")
    if len(parts) != 2:
        return None
    h = int(parts[0])
    m = int(parts[1])
    if h >= 24 or m >= 60:
        return None
    return h * 60 + m


def _rfc3339(s):
    if not s:
        return None
    s = str(s)
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        return datetime.datetime.fromisoformat(s).astimezone(datetime.timezone.utc)
    except ValueError:
        return None


def _minutes_into_window(local: int, start: int, end: int):
    length = (end + 1440 - start) % 1440
    into = (local + 1440 - start) % 1440
    if length > 0 and into < length:
        return into
    return None


def gate(args: dict, s: dict, now) -> str:
    """'' when every rule holds, else the reason (same strings as sensor_rules.rs)."""
    me = args.get("engine")
    if me and s.get("engine") and str(s.get("engine")) != str(me):
        return "another engine owns dreaming"
    window = str(args.get("window") or "01:00-06:00").split("-")
    if len(window) != 2:
        return "window malformed"
    start = _hhmm(window[0])
    end = _hhmm(window[1])
    if start is None or end is None:
        return "window malformed"
    tz = int(s.get("tz_offset_minutes") or 0)
    secs = (int(now.timestamp()) % 86400) + tz * 60
    local = (secs % 86400) // 60
    into = _minutes_into_window(local, start, end)
    if into is None:
        return "outside the window"
    idle = int(args.get("idle") or 20)
    last_turn = _rfc3339(s.get("last_root_turn_at"))
    if last_turn is not None and (now - last_turn).total_seconds() // 60 < idle:
        return "a root session turned recently"
    if int(s.get("desks_doing") or 0) > 0:
        return "a desk is doing"
    concepts = int(s.get("new_concepts_since") or 0)
    sessions = int(s.get("sessions_since") or 0)
    heavy = concepts >= int(args.get("heavy_concepts") or 300) or sessions >= int(args.get("heavy_sessions") or 40)
    window_start = now - datetime.timedelta(minutes=into)
    last_dream = _rfc3339(s.get("last_dream_at"))
    if last_dream is not None and last_dream >= window_start and not heavy:
        return "already dreamt this window"
    if concepts == 0 and sessions == 0:
        return "nothing new since the last dream"
    on_ac = s.get("on_ac")
    pct = s.get("battery_pct")
    if on_ac is True:
        power_ok = True
    elif pct is not None:
        power_ok = int(pct) >= BATTERY_MIN_PCT
    elif on_ac is False:
        power_ok = False
    else:
        power_ok = True
    if not power_ok:
        return "on battery below 40%"
    return ""


def strip_header(text: str) -> str:
    """`read_file` always starts with a `[read_file <path>: N lines, …]` line, even with numbers=False."""
    if text.startswith("[read_file "):
        return "\n".join(text.split("\n")[1:])
    return text


async def check(state: dict, args: dict) -> dict:
    try:
        text = await read_file("dreaming/signals.json", numbers=False)
        signals = json.loads(strip_header(text))
    except Exception as e:  # noqa: BLE001 — no file yet is "nothing new"
        return {"fire": False, "payload": "subject=dreaming events=no-signals " + str(e), "state": {}, "next": None}
    now = datetime.datetime.now(datetime.timezone.utc)
    why = gate(args, signals, now)
    payload = "subject=dreaming events=" + ("go" if why == "" else "hold") + "\n"
    payload = payload + ("gate: open" if why == "" else "gate: " + why)
    payload = payload + "\nnew_concepts_since=" + str(signals.get("new_concepts_since") or 0)
    payload = payload + " sessions_since=" + str(signals.get("sessions_since") or 0)
    return {"fire": why == "", "payload": payload, "state": {}, "next": None}
