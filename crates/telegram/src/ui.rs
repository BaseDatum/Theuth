//! NEEDS(COMMANDS): rich-message and inline-keyboard builders for the panels
//! and `cmd:` routing (design doc 98 §10.3). Stage 1: [`table`] is real;
//! [`action`] answers nothing and [`guard`] passes text through.

use crate::commands::{CommandCx, Outcome};

/// A `cmd:` press (`data` without the prefix): re-render the panel.
// NEEDS(COMMANDS): the panel presses. Stage 1 handles none.
pub async fn action(_cx: CommandCx<'_>, _data: &str) -> Option<Outcome> {
    None
}

/// Escapes bare `<` and `&` outside code spans and fences so Rich Markdown
/// does not read them as HTML.
// NEEDS(COMMANDS): the escape. Stage 1 returns the input unchanged.
#[must_use]
pub fn guard(md: &str) -> String {
    md.to_owned()
}

/// A GFM pipe table; the first row is the header. Cells have `|` escaped.
#[must_use]
pub fn table(rows: impl IntoIterator<Item = Vec<String>>) -> String {
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return String::new();
    };
    let line = |cells: &[String]| {
        let cells: Vec<String> = cells.iter().map(|c| c.replace('|', "\\|")).collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = line(&header);
    out.push('\n');
    out.push_str(&format!("|{}", " --- |".repeat(header.len())));
    for row in rows {
        out.push('\n');
        out.push_str(&line(&row));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_table_shape() {
        let t = table([
            vec!["a".to_owned(), "b".to_owned()],
            vec!["1|x".to_owned(), "2".to_owned()],
        ]);
        assert_eq!(t, "| a | b |\n| --- | --- |\n| 1\\|x | 2 |");
        assert_eq!(table(Vec::<Vec<String>>::new()), "");
    }
}
