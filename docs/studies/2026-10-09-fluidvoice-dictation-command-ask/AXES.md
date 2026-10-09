# AXES — FluidVoice vs theuth voice (dictation · command mode · ask-anywhere voice)

Shared numbering for every recon report. Every report uses these headings in this order, `## A1` … `## A8`,
then `## Map to the three questions`, then `## Cites self-check`. A claim about code carries a cite
`path#L<a>-L<b>` (repo-relative). "Not found" is a valid finding — write it, do not guess.

Context (the ask, 2026-10-09 16:14, Sky): "do a study of dictation and FluidVoice and determine what we can learn
from it and how we can incorporate those things into theuth. Specifically: (Q1) better dictation — ours is already
pretty good; (Q2) the command mode; (Q3) a different type of dictation mode from the Ctrl+S draw/say/ask voice
mode — where you can ask the agent anything with your voice anytime and actually see the output; we kind of already
have that in the companion app — how can we make this better with lessons from FluidVoice?"

## A1 Capture front-end
Microphone ownership and device selection, device-change handling, audio processing (echo cancellation, noise
suppression, gain — FluidVoice vendors WebRTC audio processing), startup/readiness gates, idle/unload policies,
buffering, level/spectrum metering, VAD and end-of-utterance detection, and the press → first captured sample →
first word on screen path with every latency number found.

## A2 STT engines and streaming
Which models/providers, how streaming works (chunk sizes, partial vs final text, cache/state), end-of-utterance
models (Parakeet Flash / realtime EOU), model catalog + download + install + verification + updates, memory and
unload rules, vocabulary biasing / hotwords, language selection, fallbacks (Apple Speech), file transcription.

## A3 Transcript shaping (between STT and the target)
Spoken punctuation and literal formatting (numbers, hyphens, emails, "new line", "comma"), the AI post-processing
layer (prompts, when it runs and when it is skipped — gates, local vs cloud providers, thinking-token parsing,
latency), per-app prompt sets and what context is sent (app name, selected text, screen?), custom dictionary and
AUTOMATIC pronunciation learning (correction detection → training session → matcher → replacement protection),
spoken "send" parsing, undo and quality feedback.

## A4 Delivery into the target
Typing vs paste strategies, accessibility insertion, paste verification and retries, pasteboard preservation,
target assessment/policy (secure fields, terminals, browsers), delivery-failure UX, and Write/Rewrite mode
(select text → rewrite inline: prompts, UI, confirmation).

## A5 Command mode
Trigger, UI (command window, sessions sidebar, markdown output, thinking/shimmer), how a spoken request becomes
actions (LLM with tools? shortcuts? AppleScript? terminal? app search?), model picker and catalog, confirmation and
safety, how results are shown and kept, any local HTTP/API surface, history/sessions.

## A6 Overlay, hotkey and gesture UX
Hotkey manager (hold vs toggle, double-tap lock, modifier-only chords, fn, conflicts, Esc, repeat suppression),
notch overlay vs bottom overlay, sizes/pill, live preview, waveform, which display the overlay appears on
(pointer/active screen), sounds, notifications, menu bar, onboarding tryout, theming — and exactly which state is
shown when (idle / listening / processing / enhancing / delivered / failed).

## A7 History, stats and the feedback loop
Audio history and budgets, raw-vs-enhanced transcript diff, usage stats, analytics/privacy, feedback reporting —
what loop makes dictation better over time for one user.

## A8 Engineering quality and traction
Tests, eval tooling, diagnostics/logging, updater/rollback, release cadence, repo health (stars/issues/PRs),
license and what that means for borrowing (GPL-3: techniques only, no code copied).

## Map to the three questions
Q1 better dictation · Q2 command mode · Q3 ask-the-agent-anywhere voice mode with visible output.
Per question: what they do that we do not (theirs) / what we have and lack (ours) / what users say (web).

## Cites self-check
Re-open ≥ 15 of your own cites before you finish; list `path#L — held | corrected`.
