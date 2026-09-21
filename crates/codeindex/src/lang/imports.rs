//! Import statements → `imports` rows (60 §5.2). Step 2 of 60 §16: one
//! `tree_sitter::Query` per language (Rust `use_declaration`/`mod_item`, TS/JS
//! `import_statement`/`require`, Python `import_from_statement`, Go
//! `import_spec`) executed on the same tree the tags run on. Step 1 creates the
//! `imports` table and writes no rows; `code_outline` returns `imports: []`.
