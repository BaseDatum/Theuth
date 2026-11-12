use super::*;

impl Model {

    /// The board (design doc 19 §3.3): the subtree's live rows plus errors
    /// and cancels the picker has not shown yet, in picker order. Empty =
    /// hidden.
    pub fn board(&self) -> Vec<&AgentRow> {
        let mut rows: Vec<&AgentRow> = self.agents.values().filter(|r| r.on_board()).collect();
        rows.sort_by_key(|r| rank(r));
        rows
    }
}



impl Model {
    /// One `/agents` picker or board line: [`agent_row`] plus the child's
    /// cost tag (141 R3 T13: watch rows carry their money like the desktop).
    /// The tag is measured first and the row fitted to the rest (R4 T13b), so
    /// the detail is what gets cut, never the money.
    pub fn board_row(&self, a: &AgentRow, width: u16) -> String {
        let tag = self.component_text(crate::components::Surface::SessionBadge(a.info.id));
        let fit =
            width.saturating_sub(u16::try_from(crate::components::search_attach::state::cells(&tag)).unwrap_or(u16::MAX));
        format!("{}{tag}", agent_row(a, self.spinner(), fit))
    }

}
