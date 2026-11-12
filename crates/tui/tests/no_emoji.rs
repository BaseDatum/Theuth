//! Strings the desktop shows as chrome carry no emoji: the desktop draws lucide icons from kinds/enums
//! (NO-EMOJI; the desktop's own tree is scanned by `crates/desktop/ui/src/lib/no-emoji.test.ts`). Glyphs in
//! terminal text — the TUI, and `surface`'s text lowerings for the TUI, Signal and export — are allowed
//! (Sky, 2026-10-07: "glyphs in the tui are acceptable, just no emojis in desktop"), so only the crates whose
//! strings reach the desktop as chrome are scanned: see `CRATES`. Each offender is named as `file:line`;
//! a line that legitimately needs one ends with `// no-emoji: allow`.

use std::fs;
use std::path::{Path, PathBuf};

const CRATES: &[&str] = &[
    "agent",
    "rpc",
    "rpc-core",
    "cells",
    "mcp-server",
    "sandbox",
    "computer",
    "codeindex",
    "dreaming",
    "meeting",
    "talk",
    "autoresearch",
    "nodes",
    "schedule",
    "secrets",
    "roster",
];

/// `Extended_Pictographic` ∪ `Emoji_Presentation` (Unicode 16) minus the arrows block and ™, which are
/// typography; plus the emoji variation selector and the keycap combiner.
fn is_emoji(c: char) -> bool {
    let c = c as u32;
    matches!(c,
        0x1F000..=0x1FAFF | 0xFE0F | 0x20E3 | 0x203C | 0x2049 | 0x2139
        | 0x231A..=0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2
        | 0x25AA..=0x25AB | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE
        | 0x2600..=0x2604 | 0x260E | 0x2611 | 0x2614..=0x2615 | 0x2618 | 0x261D | 0x2620 | 0x2622..=0x2623
        | 0x2626 | 0x262A | 0x262E..=0x262F | 0x2638..=0x263A | 0x2640 | 0x2642 | 0x2648..=0x2653
        | 0x265F..=0x2660 | 0x2663 | 0x2665..=0x2666 | 0x2668 | 0x267B | 0x267E..=0x267F | 0x2692..=0x2697
        | 0x2699 | 0x269B..=0x269C | 0x26A0..=0x26A1 | 0x26A7 | 0x26AA..=0x26AB | 0x26B0..=0x26B1
        | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26C8 | 0x26CE..=0x26CF | 0x26D1 | 0x26D3..=0x26D4
        | 0x26E9..=0x26EA | 0x26F0..=0x26F5 | 0x26F7..=0x26FA | 0x26FD
        | 0x2702 | 0x2705 | 0x2708..=0x270D | 0x270F | 0x2712 | 0x2714 | 0x2716 | 0x271D | 0x2721 | 0x2728
        | 0x2733..=0x2734 | 0x2744 | 0x2747 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2763..=0x2764
        | 0x2795..=0x2797 | 0x27A1 | 0x27B0 | 0x27BF | 0x2934..=0x2935 | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C
        | 0x2B50 | 0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_emoji_in_desktop_chrome_strings() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for c in CRATES {
        rs_files(&crates.join(c).join("src"), &mut files);
    }
    let mut hits = Vec::new();
    for f in &files {
        for (i, line) in fs::read_to_string(f).unwrap().lines().enumerate() {
            if line.trim_end().ends_with("// no-emoji: allow") {
                continue;
            }
            let found: String = line.chars().filter(|c| is_emoji(*c)).collect();
            if !found.is_empty() {
                hits.push(format!(
                    "{}:{}  {found}",
                    f.strip_prefix(crates).unwrap().display(),
                    i + 1
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} emoji line(s):\n{}",
        hits.len(),
        hits.join("\n")
    );
    assert!(is_emoji('⏳') && is_emoji('💭') && !is_emoji('✗') && !is_emoji('☐') && !is_emoji('↔'));
}
