-- 183 §5.6 (W-11): machine facts for pickers, cards and the prompt header.
-- '' from engines older than these columns; replicates with the row (an
-- older engine skips the unknown columns with a warning, `replica::apply`).
ALTER TABLE engine_stats ADD COLUMN os      TEXT NOT NULL DEFAULT '';  -- std::env::consts::OS: macos | linux | windows
ALTER TABLE engine_stats ADD COLUMN os_name TEXT NOT NULL DEFAULT '';  -- display: macOS | Arch Linux | Ubuntu | Windows
ALTER TABLE engine_stats ADD COLUMN arch    TEXT NOT NULL DEFAULT '';  -- std::env::consts::ARCH: aarch64 | x86_64
ALTER TABLE engine_stats ADD COLUMN host    TEXT NOT NULL DEFAULT '';  -- hostname, distinct from the mesh name
