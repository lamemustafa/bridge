#[test]
fn client_preference_commands_are_mirror_and_keychain_free() {
    let commands = include_str!("commands/all_clients.rs");
    let start = commands
        .find("pub fn load_client_group_labels")
        .expect("group-label load command");
    let end = commands[start..]
        .find("pub async fn prepare_client_group_label_migration")
        .map(|offset| start + offset)
        .expect("end of group-label commands");
    let group_label_load_command = &commands[start..end];
    let sort_start = commands
        .find("pub fn load_client_sort_preference")
        .expect("client-sort load command");
    // The sort-preference commands close commands/all_clients.rs, so the span runs to its end.
    let client_preference_commands = &commands[sort_start..];

    assert!(group_label_load_command.contains("app_config_dir"));
    assert!(!group_label_load_command.contains("LazyTallyMirror"));
    assert!(!group_label_load_command.contains("keyring"));
    assert!(client_preference_commands.contains("load_client_sort_preference"));
    assert!(client_preference_commands.contains("save_client_sort_preference"));
    assert!(client_preference_commands.contains("app_config_dir"));
    assert!(!client_preference_commands.contains("LazyTallyMirror"));
    assert!(!client_preference_commands.contains("keyring"));
}
