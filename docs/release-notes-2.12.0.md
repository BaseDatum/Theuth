# theuth 2.12.0 — release notes (draft)

## What's new

### The owner's double (design 126)
- **double:** a local model of your own decisions. With `double.enabled: true` (restart) it predicts your answer to every approval and `ask_user` card before you see it (a chip `double · Allow · 91 %` on desktop, TUI, Slack, Signal and Telegram), learns from what you then do, and climbs a per-class ladder: L0 observe → L1 pre-select → L2 answer for you after a 30 s grace you can take over. Destructive classes never pass L1; mutating classes need a per-class opt-in; secrets, handoffs and MFA are never seen. Controls in Settings › Double, `/double …` in the TUI and the `double_*` chat functions; training runs locally or on your own GPU mesh node (never a sandbox VM); everything stays in `~/.theuth/double/`. Needs the 0033 journal and `tools.approvals` on for approval cards. Guide: `docs/double.md`.
