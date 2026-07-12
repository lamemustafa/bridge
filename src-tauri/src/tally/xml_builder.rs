pub enum ImportAction {
    Create,
    Alter,
}

impl ImportAction {
    pub fn as_tally_action(&self) -> &'static str {
        match self {
            ImportAction::Create => "Create",
            ImportAction::Alter => "Alter",
        }
    }
}
