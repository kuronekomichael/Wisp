//! Resolution and persistence for the user-configurable model storage parent.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SETTING_FILE: &str = "model-storage.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelStorageRoots {
    pub models: PathBuf,
    pub embed_models: PathBuf,
    pub custom_models: PathBuf,
}

impl ModelStorageRoots {
    pub fn under(parent: &Path) -> Self {
        Self {
            models: parent.join("models"),
            embed_models: parent.join("embed-models"),
            custom_models: parent.join("custom-models"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelStorageSetting {
    pub parent: Option<PathBuf>,
}

impl ModelStorageSetting {
    pub fn configured_parent(&self, app_data: &Path) -> PathBuf {
        self.parent.clone().unwrap_or_else(|| app_data.to_owned())
    }
    pub fn pending_restart(&self, active_parent: &Path, app_data: &Path) -> bool {
        self.configured_parent(app_data) != active_parent
    }
}

pub fn settings_path(app_data: &Path) -> PathBuf {
    app_data.join(SETTING_FILE)
}
pub fn load_setting(app_data: &Path) -> Result<ModelStorageSetting, String> {
    let path = settings_path(app_data);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid model storage setting: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ModelStorageSetting::default()),
        Err(e) => Err(format!("cannot read model storage setting: {e}")),
    }
}

pub fn validate_parent(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("model storage path must be absolute".into());
    }
    let metadata =
        fs::metadata(path).map_err(|e| format!("model storage directory unavailable: {e}"))?;
    if !metadata.is_dir() {
        return Err("model storage path is not a directory".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = path.join(format!(
        ".wisp-storage-probe-{}-{stamp}",
        std::process::id()
    ));
    let _entries =
        fs::read_dir(path).map_err(|e| format!("model storage directory is not readable: {e}"))?;
    let mut probe = ProbeFile::create(&probe)
        .map_err(|e| format!("model storage directory is not writable: {e}"))?;
    probe
        .file
        .as_mut()
        .expect("probe file is open")
        .write_all(b"wisp")
        .map_err(|e| format!("model storage directory is not writable: {e}"))?;
    probe
        .file
        .as_mut()
        .expect("probe file is open")
        .sync_all()
        .map_err(|e| format!("model storage directory is not writable: {e}"))?;
    drop(probe);
    Ok(())
}

struct ProbeFile {
    path: PathBuf,
    file: Option<fs::File>,
}
impl ProbeFile {
    fn create(path: &Path) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
        })
    }
}
impl Drop for ProbeFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct TempFile {
    path: PathBuf,
    file: Option<fs::File>,
    cleanup: bool,
}
impl TempFile {
    fn create(parent: &Path, stem: &str) -> Result<Self, String> {
        for _ in 0..100 {
            let seq = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = parent.join(format!(".{stem}-{}-{seq}", std::process::id()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        cleanup: true,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("could not allocate unique temporary file".into())
    }
    fn file_mut(&mut self) -> &mut fs::File {
        self.file.as_mut().expect("temporary file is open")
    }
    fn close(&mut self) {
        self.file.take();
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn disarm(&mut self) {
        self.cleanup = false;
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        self.file.take();
        if self.cleanup {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn validate_child_writable(path: &Path) -> Result<(), String> {
    let mut probe = TempFile::create(path, "wisp-storage-probe")
        .map_err(|e| format!("model storage child is not writable: {e}"))?;
    probe
        .file_mut()
        .write_all(b"wisp")
        .map_err(|e| format!("model storage child is not writable: {e}"))?;
    probe.close();
    Ok(())
}

/// Checks an existing child without creating it, including effective read/write access.
fn validate_child(path: &Path) -> Result<(), String> {
    if !fs::metadata(path)
        .map_err(|e| format!("model storage child unavailable: {e}"))?
        .is_dir()
    {
        return Err(format!(
            "model storage child is not a directory: {}",
            path.display()
        ));
    }
    fs::read_dir(path).map_err(|e| format!("model storage child is not readable: {e}"))?;
    validate_child_writable(path)
}

/// Validates a selection without preparing its children or retargeting the running stores.
fn validate_destination(parent: &Path) -> Result<(), String> {
    validate_parent(parent)?;
    let roots = ModelStorageRoots::under(parent);
    for path in [&roots.models, &roots.embed_models, &roots.custom_models] {
        match fs::metadata(path) {
            Ok(_) => validate_child(path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("model storage child unavailable: {e}")),
        }
    }
    Ok(())
}

pub fn prepare_roots(parent: &Path) -> Result<ModelStorageRoots, String> {
    validate_parent(parent)?;
    let roots = ModelStorageRoots::under(parent);
    for path in [&roots.models, &roots.embed_models, &roots.custom_models] {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                fs::read_dir(path).map_err(|e| format!("model storage child unavailable: {e}"))?;
            }
            Ok(_) => {
                return Err(format!(
                    "model storage child is not a directory: {}",
                    path.display()
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
                .map_err(|e| format!("cannot prepare model storage child: {e}"))?,
            Err(e) => return Err(format!("model storage child unavailable: {e}")),
        }
        validate_child_writable(path)?;
    }
    Ok(roots)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelStorageStatus {
    pub active_parent: PathBuf,
    pub saved_parent: Option<PathBuf>,
    pub pending_restart: bool,
    pub roots: ModelStorageRoots,
    pub error: Option<String>,
}

/// Keeps the active root immutable for this run while allowing a validated next-start setting.
pub struct ModelStorageService {
    app_data: PathBuf,
    active_parent: PathBuf,
    saved: ModelStorageSetting,
    status: ModelStorageStatus,
    config_error: Option<String>,
}
impl ModelStorageService {
    pub fn start(app_data: &Path) -> Result<Self, String> {
        fs::create_dir_all(app_data)
            .map_err(|e| format!("cannot prepare app data directory: {e}"))?;
        let (saved, config_error) = match load_setting(app_data) {
            Ok(setting) => (setting, None),
            Err(error) => (ModelStorageSetting::default(), Some(error)),
        };
        let active_parent = saved.configured_parent(app_data);
        let (roots, error) = match if config_error.is_some() {
            Err(config_error.clone().unwrap())
        } else {
            prepare_roots(&active_parent)
        } {
            Ok(roots) => (roots, config_error.clone()),
            Err(storage_error) => (
                ModelStorageRoots::under(&active_parent),
                Some(config_error.clone().unwrap_or(storage_error)),
            ),
        };
        let status = ModelStorageStatus {
            active_parent: active_parent.clone(),
            saved_parent: saved.parent.clone(),
            pending_restart: false,
            roots,
            error,
        };
        Ok(Self {
            app_data: app_data.to_owned(),
            active_parent,
            saved,
            status,
            config_error,
        })
    }
    pub fn active_parent(&self) -> &Path {
        &self.active_parent
    }
    pub fn status(&self) -> &ModelStorageStatus {
        &self.status
    }
    pub fn check_available(&mut self) -> Result<(), String> {
        let result = (|| {
            if let Some(error) = &self.config_error {
                return Err(error.clone());
            }
            validate_parent(&self.active_parent)?;
            for path in [
                &self.status.roots.models,
                &self.status.roots.embed_models,
                &self.status.roots.custom_models,
            ] {
                validate_child(path)?;
            }
            Ok(())
        })();
        self.status.error = result.as_ref().err().cloned();
        result
    }
    pub fn set_next_start(&mut self, parent: Option<&Path>) -> Result<(), String> {
        let saved = save_setting(&self.app_data, parent)?;
        self.saved = saved;
        self.refresh_pending();
        Ok(())
    }
    pub fn reset_to_default(&mut self) -> Result<(), String> {
        self.set_next_start(None)
    }
    fn refresh_pending(&mut self) {
        self.status.saved_parent = self.saved.parent.clone();
        self.status.pending_restart = self.config_error.is_some()
            || self
                .saved
                .pending_restart(&self.active_parent, &self.app_data);
    }
}

pub fn save_setting(app_data: &Path, parent: Option<&Path>) -> Result<ModelStorageSetting, String> {
    let setting = ModelStorageSetting {
        parent: parent.map(Path::to_owned),
    };
    if let Some(parent) = parent {
        validate_destination(parent)?;
    }
    fs::create_dir_all(app_data).map_err(|e| format!("cannot create app data directory: {e}"))?;
    let path = settings_path(app_data);
    let bytes = serde_json::to_vec(&setting).map_err(|e| e.to_string())?;
    let mut temp = TempFile::create(app_data, "model-storage")
        .map_err(|e| format!("cannot create settings temporary file: {e}"))?;
    temp.file_mut()
        .write_all(&bytes)
        .map_err(|e| format!("cannot write settings: {e}"))?;
    temp.file_mut()
        .sync_all()
        .map_err(|e| format!("cannot sync settings: {e}"))?;
    temp.close();
    fs::rename(temp.path(), &path)
        .map_err(|e| format!("cannot save model storage setting: {e}"))?;
    // Rename consumes the temp pathname; prevent Drop from touching a later file at this name.
    temp.disarm();
    Ok(setting)
}

#[cfg(test)]
mod tests {
    #[test]
    fn resetting_invalid_config_requires_restart_even_at_default_root() {
        let app = temp();
        fs::write(settings_path(&app), b"not json").unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        service.reset_to_default().unwrap();
        assert!(service.status().pending_restart);
        assert!(service.check_available().is_err());
        assert!(ModelStorageService::start(&app)
            .unwrap()
            .check_available()
            .is_ok());
        fs::remove_dir_all(app).unwrap();
    }

    #[test]
    fn setting_validates_existing_children_without_preparing_new_ones() {
        let app = temp();
        let chosen = temp();
        save_setting(&app, Some(&chosen)).unwrap();
        assert!(!chosen.join("models").exists());
        let invalid = temp();
        fs::write(invalid.join("models"), b"not a directory").unwrap();
        assert!(save_setting(&app, Some(&invalid)).is_err());
        assert_eq!(load_setting(&app).unwrap().parent, Some(chosen.clone()));
        assert!(!invalid.join("embed-models").exists());
        for dir in [app, chosen, invalid] {
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn restarted_service_uses_all_selected_children_and_leaves_app_data_intact() {
        let app = temp();
        let chosen = temp();
        fs::write(app.join("library.db"), b"database").unwrap();
        fs::write(app.join("cloud-keys.json"), b"keys").unwrap();
        let mut current = ModelStorageService::start(&app).unwrap();
        current.set_next_start(Some(&chosen)).unwrap();
        assert_eq!(current.status().roots, ModelStorageRoots::under(&app));
        let next = ModelStorageService::start(&app).unwrap();
        assert_eq!(next.status().roots, ModelStorageRoots::under(&chosen));
        assert_eq!(fs::read(app.join("library.db")).unwrap(), b"database");
        assert_eq!(fs::read(app.join("cloud-keys.json")).unwrap(), b"keys");
        assert!(!chosen.join("library.db").exists());
        assert!(!chosen.join("cloud-keys.json").exists());
        assert!(settings_path(&app).is_file());
        current.reset_to_default().unwrap();
        assert_eq!(
            ModelStorageService::start(&app).unwrap().active_parent(),
            app
        );
        fs::remove_dir_all(app).unwrap();
        fs::remove_dir_all(chosen).unwrap();
    }

    #[test]
    fn malformed_config_blocks_existing_defaults_until_next_start() {
        let app = temp();
        prepare_roots(&app).unwrap();
        fs::write(settings_path(&app), b"not json").unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        assert!(service.check_available().is_err());
        let error = service.status().error.clone();
        let chosen = temp();
        service.set_next_start(Some(&chosen)).unwrap();
        assert_eq!(service.active_parent(), app);
        assert!(service.check_available().is_err());
        service.reset_to_default().unwrap();
        assert!(service.check_available().is_err());
        assert_eq!(service.status().error, error);
        assert!(ModelStorageService::start(&app)
            .unwrap()
            .check_available()
            .is_ok());
        fs::remove_dir_all(app).unwrap();
        fs::remove_dir_all(chosen).unwrap();
    }

    #[test]
    fn reconnect_clears_availability_error_and_probes_leave_no_files() {
        let app = temp();
        let chosen = temp();
        save_setting(&app, Some(&chosen)).unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        let offline = chosen.with_extension("offline");
        fs::rename(&chosen, &offline).unwrap();
        assert!(service.check_available().is_err());
        assert!(!chosen.exists());
        fs::rename(&offline, &chosen).unwrap();
        service.check_available().unwrap();
        assert!(service.status().error.is_none());
        for path in [
            &chosen,
            &chosen.join("models"),
            &chosen.join("embed-models"),
            &chosen.join("custom-models"),
        ] {
            assert!(fs::read_dir(path).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("probe")));
        }
        fs::remove_dir_all(app).unwrap();
        fs::remove_dir_all(chosen).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn startup_rejects_effectively_read_only_children() {
        use std::os::unix::fs::PermissionsExt;
        let parent = temp();
        let roots = prepare_roots(&parent).unwrap();
        fs::set_permissions(&roots.models, fs::Permissions::from_mode(0o555)).unwrap();
        // Root / privileged runners may bypass mode bits; test effective permissions instead.
        let writable = TempFile::create(&roots.models, "permission-test").is_ok();
        if !writable {
            assert!(prepare_roots(&parent).is_err());
            assert!(ModelStorageService::start(&parent)
                .unwrap()
                .status()
                .error
                .is_some());
        }
        fs::set_permissions(&roots.models, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn service_keeps_active_root_when_saved_setting_changes() {
        let app = temp();
        let selected = temp();
        let mut service = ModelStorageService::start(&app).unwrap();
        assert_eq!(service.active_parent(), app);
        service.set_next_start(Some(&selected)).unwrap();
        assert_eq!(service.active_parent(), app);
        assert!(service.status().pending_restart);
        assert_eq!(service.status().saved_parent, Some(selected.clone()));
        let _ = fs::remove_dir_all(app);
        let _ = fs::remove_dir_all(selected);
    }

    #[test]
    fn startup_reports_malformed_setting_and_reset_recovers() {
        let app = temp();
        fs::write(settings_path(&app), b"not json").unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        assert!(service.status().error.is_some());
        service.reset_to_default().unwrap();
        assert!(service.check_available().is_err());
        assert!(service.status().error.is_some());
        assert_eq!(load_setting(&app).unwrap(), ModelStorageSetting::default());
        let _ = fs::remove_dir_all(app);
    }

    #[test]
    fn startup_prepares_children_only_for_valid_existing_parent() {
        let app = temp();
        let chosen = temp();
        save_setting(&app, Some(&chosen)).unwrap();
        let service = ModelStorageService::start(&app).unwrap();
        assert!(service.status().error.is_none());
        let roots = ModelStorageRoots::under(&chosen);
        assert!(
            roots.models.is_dir() && roots.embed_models.is_dir() && roots.custom_models.is_dir()
        );
        let _ = fs::remove_dir_all(app);
        let _ = fs::remove_dir_all(chosen);
    }

    #[test]
    fn unavailable_startup_does_not_recreate_chosen_parent() {
        let app = temp();
        let missing = app.join("offline");
        fs::write(
            settings_path(&app),
            serde_json::to_vec(&ModelStorageSetting {
                parent: Some(missing.clone()),
            })
            .unwrap(),
        )
        .unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        assert!(service.status().error.is_some());
        assert!(service.check_available().is_err());
        assert!(service.status().error.is_some());
        assert!(!missing.exists());
        let _ = fs::remove_dir_all(app);
    }

    #[test]
    fn availability_check_detects_disconnected_root_without_recreating_it() {
        let app = temp();
        let chosen = temp();
        save_setting(&app, Some(&chosen)).unwrap();
        let mut service = ModelStorageService::start(&app).unwrap();
        fs::remove_dir_all(&chosen).unwrap();
        assert!(service.check_available().is_err());
        assert!(service.status().error.is_some());
        assert!(!chosen.exists());
        let _ = fs::remove_dir_all(app);
    }

    #[test]
    fn storage_children_must_be_directories() {
        let parent = temp();
        fs::write(parent.join("models"), b"file").unwrap();
        assert!(prepare_roots(&parent).is_err());
        let _ = fs::remove_dir_all(parent);
    }

    use super::*;
    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wisp-storage-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        p
    }
    #[test]
    fn roots_and_restart_resolution() {
        let app = temp();
        let chosen = temp();
        let setting = ModelStorageSetting {
            parent: Some(chosen.clone()),
        };
        assert_eq!(
            ModelStorageRoots::under(&chosen).embed_models,
            chosen.join("embed-models")
        );
        assert!(setting.pending_restart(&app, &app));
        assert!(!setting.pending_restart(&chosen, &app));
        let _ = fs::remove_dir_all(app);
        let _ = fs::remove_dir_all(chosen);
    }
    #[test]
    fn invalid_setting_does_not_replace_existing() {
        let app = temp();
        let chosen = temp();
        save_setting(&app, Some(&chosen)).unwrap();
        let missing = chosen.join("missing");
        assert!(save_setting(&app, Some(&missing)).is_err());
        assert_eq!(load_setting(&app).unwrap().parent, Some(chosen.clone()));
        let _ = fs::remove_dir_all(app);
        let _ = fs::remove_dir_all(chosen);
    }
    #[test]
    fn temporary_settings_files_are_unique_and_cleaned_on_failure_path() {
        let parent = temp();
        let first = TempFile::create(&parent, "probe").unwrap();
        let first_path = first.path().to_owned();
        let second = TempFile::create(&parent, "probe").unwrap();
        assert_ne!(first.path(), second.path());
        drop(first);
        assert!(!first_path.exists());
        let second_path = second.path().to_owned();
        drop(second);
        assert!(!second_path.exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn validates_absolute_existing_directory_and_rejects_files_and_relative_paths() {
        let parent = temp();
        let file = parent.join("file");
        fs::write(&file, b"x").unwrap();
        assert!(validate_parent(&parent).is_ok());
        assert!(validate_parent(&file).is_err());
        assert!(validate_parent(Path::new("relative")).is_err());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn unavailable_parent_is_not_created() {
        let app = temp();
        let missing = app.join("not-created");
        assert!(validate_parent(&missing).is_err());
        assert!(!missing.exists());
        let _ = fs::remove_dir_all(app);
    }
}
