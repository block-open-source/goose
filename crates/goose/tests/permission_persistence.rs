use goose::config::permission::{PermissionConfig, PermissionLevel, PermissionManager};
use std::collections::HashMap;

#[test]
fn stale_manager_cannot_restore_revoked_permission() {
    let config_dir = tempfile::tempdir().unwrap();
    let current_manager = PermissionManager::new(config_dir.path().to_path_buf());

    current_manager.update_user_permission("dangerous_tool", PermissionLevel::AlwaysAllow);
    let stale_manager = PermissionManager::new(config_dir.path().to_path_buf());

    current_manager.update_user_permission("dangerous_tool", PermissionLevel::NeverAllow);
    stale_manager.update_user_permission("unrelated_tool", PermissionLevel::AlwaysAllow);

    assert_eq!(
        stale_manager.get_user_permission("dangerous_tool"),
        Some(PermissionLevel::NeverAllow)
    );

    let reloaded_manager = PermissionManager::new(config_dir.path().to_path_buf());
    assert_eq!(
        reloaded_manager.get_user_permission("dangerous_tool"),
        Some(PermissionLevel::NeverAllow)
    );
    assert_eq!(
        reloaded_manager.get_user_permission("unrelated_tool"),
        Some(PermissionLevel::AlwaysAllow)
    );
}

#[test]
fn permission_updates_persist_across_manager_instances() {
    let config_dir = tempfile::tempdir().unwrap();
    let manager = PermissionManager::new(config_dir.path().to_path_buf());

    manager.update_user_permission("user_tool", PermissionLevel::AskBefore);
    manager.update_smart_approve_permission("smart_tool", PermissionLevel::NeverAllow);

    let reloaded_manager = PermissionManager::new(config_dir.path().to_path_buf());
    assert_eq!(
        reloaded_manager.get_user_permission("user_tool"),
        Some(PermissionLevel::AskBefore)
    );
    assert_eq!(
        reloaded_manager.get_smart_approve_permission("smart_tool"),
        Some(PermissionLevel::NeverAllow)
    );
}

#[cfg(unix)]
#[test]
fn permission_updates_atomically_replace_storage_file() {
    use std::os::unix::fs::MetadataExt;

    let config_dir = tempfile::tempdir().unwrap();
    let manager = PermissionManager::new(config_dir.path().to_path_buf());
    manager.update_user_permission("first_tool", PermissionLevel::AlwaysAllow);

    let permission_path = manager.get_config_path();
    let old_file = std::fs::File::open(permission_path).unwrap();
    let old_inode = old_file.metadata().unwrap().ino();

    manager.update_user_permission("second_tool", PermissionLevel::AskBefore);

    assert_ne!(std::fs::metadata(permission_path).unwrap().ino(), old_inode);
    let old_values: HashMap<String, PermissionConfig> = serde_yaml::from_reader(old_file).unwrap();
    assert!(old_values["user"]
        .always_allow
        .contains(&"first_tool".to_string()));
    assert!(!old_values["user"]
        .ask_before
        .contains(&"second_tool".to_string()));

    let current_manager = PermissionManager::new(config_dir.path().to_path_buf());
    assert_eq!(
        current_manager.get_user_permission("first_tool"),
        Some(PermissionLevel::AlwaysAllow)
    );
    assert_eq!(
        current_manager.get_user_permission("second_tool"),
        Some(PermissionLevel::AskBefore)
    );
}

#[cfg(unix)]
#[test]
fn permission_updates_preserve_storage_symlink() {
    use std::os::unix::fs::symlink;

    let config_dir = tempfile::tempdir().unwrap();
    let permission_path = config_dir.path().join("permission.yaml");
    let target_path = config_dir.path().join("managed-permissions.yaml");
    std::fs::write(&target_path, "{}\n").unwrap();
    symlink("managed-permissions.yaml", &permission_path).unwrap();

    let manager = PermissionManager::new(config_dir.path().to_path_buf());
    manager.update_user_permission("user_tool", PermissionLevel::AlwaysAllow);

    assert!(std::fs::symlink_metadata(&permission_path)
        .unwrap()
        .file_type()
        .is_symlink());
    let persisted = PermissionManager::new(config_dir.path().to_path_buf());
    assert_eq!(
        persisted.get_user_permission("user_tool"),
        Some(PermissionLevel::AlwaysAllow)
    );
}

#[test]
fn permission_removal_and_clear_persist() {
    let config_dir = tempfile::tempdir().unwrap();
    let manager = PermissionManager::new(config_dir.path().to_path_buf());

    manager.update_user_permission("git__status", PermissionLevel::AlwaysAllow);
    manager.update_user_permission("github__status", PermissionLevel::AskBefore);
    manager.remove_extension("git");

    let reloaded_manager = PermissionManager::new(config_dir.path().to_path_buf());
    assert_eq!(reloaded_manager.get_user_permission("git__status"), None);
    assert_eq!(
        reloaded_manager.get_user_permission("github__status"),
        Some(PermissionLevel::AskBefore)
    );

    reloaded_manager.clear_permissions();
    let cleared_manager = PermissionManager::new(config_dir.path().to_path_buf());
    assert!(cleared_manager.get_permission_names().is_empty());
}
