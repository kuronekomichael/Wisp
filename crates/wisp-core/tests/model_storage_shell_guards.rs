//! Integration-tier source drift checks: native dispatchers and their frontend contract.
//! Runtime filesystem behavior is tested through the production core service separately.
const SHELL: &str = include_str!("../../../app/src-tauri/src/lib.rs");

fn body(name: &str) -> &str {
    let start = SHELL.find(&format!("fn {name}(")).unwrap();
    let body = &SHELL[start..];
    &body[..body.find("\n}\n").unwrap()]
}

#[test]
fn local_disk_dispatchers_keep_storage_guards() {
    for name in [
        "resolve_local_model",
        "import_file_set",
        "import_custom_model_blocking",
        "download_model",
        "download_coreml",
        "select_model",
        "remove_model",
        "download_embedding_model",
        "delete_embedding_model",
        "local_support_model_dir",
    ] {
        assert!(
            body(name).contains("check_model_storage()"),
            "unguarded {name}"
        );
    }
    for operation in [
        "ensure_with_progress",
        "ensure_coreml_with_progress",
        "download_model_with",
    ] {
        let end = SHELL.find(&format!("{operation}(")).unwrap();
        let start = SHELL[..end].rfind("spawn_blocking(move ||").unwrap();
        assert!(
            SHELL[start..end].contains("check_model_storage()"),
            "unguarded worker {operation}"
        );
    }
}

#[test]
fn support_and_local_embedding_guards_do_not_block_cloud_branches() {
    for name in ["start_session_blocking", "transcribe_file"] {
        assert!(body(name).contains("local_support_model_dir("));
    }
    for name in ["resolve_embedder", "apply_embedding_model"] {
        let body = body(name);
        let guard = body.find("check_model_storage()").unwrap();
        let local_branch = body.find("wisp_embed::catalog_model(").unwrap();
        assert!(
            local_branch < guard,
            "cloud branch must remain independent: {name}"
        );
    }
}

#[test]
fn frontend_uses_reported_paths_and_next_start_commands() {
    let settings = include_str!("../../../app/src/lib/Settings.svelte");
    let page = include_str!("../../../app/src/routes/+page.svelte");
    for command in [
        "get_model_storage_status",
        "set_model_storage_parent",
        "open_model_storage_location",
    ] {
        assert!(settings.contains(command), "missing UI command {command}");
        assert!(SHELL.contains(command), "missing backend command {command}");
    }
    assert!(
        !settings.contains("appDataDir()"),
        "UI must not reconstruct model roots"
    );
    for field in [
        "activeParent",
        "savedParent",
        "pendingRestart",
        "embedModels",
        "customModels",
    ] {
        assert!(settings.contains(field), "missing UI storage field {field}");
    }
    assert!(
        page.contains("get_model_storage_status"),
        "startup error must be surfaced"
    );
    let capabilities = include_str!("../../../app/src-tauri/capabilities/default.json");
    assert!(
        !capabilities.contains("fs:"),
        "do not expose general frontend filesystem access"
    );
    assert!(!capabilities.contains("\"path\": \"**\""));
}
