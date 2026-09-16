//! Desktop commands for the All Clients screen's local configuration: operator-owned filing
//! labels, their read-only migration plan, and the all-client sort preference. None of them
//! reads Tally; only the migration planner may open the local mirror.
use crate::client_group_label_migration::{
    classify_client_group_label_migration, ClientGroupLabelMigrationPlan,
};
use crate::client_groups;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

/// Reads operator-owned filing labels from ordinary application configuration.
///
/// The helper deliberately remains display-safe for a missing, empty, corrupt,
/// or unavailable file, while returning a typed degradation reason when one
/// exists. It receives no mirror state, so this command cannot initialise
/// SQLCipher or resolve a keychain key.
#[tauri::command]
pub fn load_client_group_labels(app: AppHandle) -> client_groups::ClientGroupLabelsLoad {
    let Ok(directory) = app.path().app_config_dir() else {
        return client_groups::ClientGroupLabelsLoad {
            labels: client_groups::ClientGroupLabels::new(),
            degradation_reason: Some(client_groups::ClientGroupLabelsDegradationReason::Read),
        };
    };
    client_groups::load_with_degradation(&directory)
}

/// A safe, typed failure returned by the read-only label-migration planner.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "code")]
pub enum ClientGroupLabelMigrationPreparationError {
    ConfigurationUnavailable,
    LabelsUnavailable,
    MirrorUnavailable,
    PersistedProfilesUnavailable,
}

pub(super) fn load_client_group_labels_for_migration(
    directory: &std::path::Path,
) -> Result<client_groups::ClientGroupLabels, ClientGroupLabelMigrationPreparationError> {
    client_groups::try_load(directory)
        .map_err(|_| ClientGroupLabelMigrationPreparationError::LabelsUnavailable)
}

pub(super) async fn prepare_client_group_label_migration_from_labels<F, Fut>(
    labels: client_groups::ClientGroupLabels,
    open_mirror_and_load_profiles: F,
) -> Result<ClientGroupLabelMigrationPlan, ClientGroupLabelMigrationPreparationError>
where
    F: FnOnce(Vec<String>) -> Fut,
    Fut: std::future::Future<
        Output = Result<
            Vec<crate::db::tally_mirror::ClientGroupLabelMigrationProfile>,
            ClientGroupLabelMigrationPreparationError,
        >,
    >,
{
    if labels.is_empty() {
        return Ok(classify_client_group_label_migration(&labels, &[]));
    }

    let raw_guids = labels.keys().cloned().collect::<Vec<_>>();
    let profiles = open_mirror_and_load_profiles(raw_guids).await?;
    Ok(classify_client_group_label_migration(&labels, &profiles))
}

/// Explicitly prepares a read-only migration plan. Unlike ordinary label
/// reads and saves, this operator-requested command may initialise the mirror
/// to inspect durable observed-company history; it never calls the label
/// writer. It fails closed if the v1 label file cannot be read, so a phase-2
/// consumer can never treat unread local input as an empty migration. An
/// absent v1 label file returns an empty plan before the mirror/keychain path.
#[tauri::command]
pub async fn prepare_client_group_label_migration(
    app: AppHandle,
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<ClientGroupLabelMigrationPlan, ClientGroupLabelMigrationPreparationError> {
    let labels = app
        .path()
        .app_config_dir()
        .map_err(|_| ClientGroupLabelMigrationPreparationError::ConfigurationUnavailable)
        .and_then(|directory| load_client_group_labels_for_migration(&directory))?;
    prepare_client_group_label_migration_from_labels(labels, |raw_guids| async move {
        let mirror = mirror
            .get()
            .await
            .map_err(|_| ClientGroupLabelMigrationPreparationError::MirrorUnavailable)?;
        mirror
            .persisted_company_profiles_for_client_group_label_migration(&raw_guids)
            .await
            .map_err(|_| ClientGroupLabelMigrationPreparationError::PersistedProfilesUnavailable)
    })
    .await
}

/// Reads the optional all-client sort preference from ordinary application
/// configuration. Like group labels, it never initialises the Tally mirror.
#[tauri::command]
pub fn load_client_sort_preference(app: AppHandle) -> Option<client_groups::ClientSortPreference> {
    let Ok(directory) = app.path().app_config_dir() else {
        return None;
    };
    client_groups::load_sort_preference(&directory)
}

#[derive(Debug, Deserialize)]
pub struct SaveClientGroupLabelRequest {
    pub company_key: String,
    pub label: String,
}

/// Saves one operator-owned filing label without accessing the Tally mirror.
#[tauri::command]
pub fn save_client_group_label(
    app: AppHandle,
    request: SaveClientGroupLabelRequest,
) -> Result<(), String> {
    if request.company_key.trim().is_empty() {
        return Err("Bridge could not identify the company for this group label.".to_string());
    }
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|_| "Bridge could not locate its local group-label configuration.".to_string())?;
    client_groups::save_label(&directory, &request.company_key, &request.label)
        .map_err(|_| "Bridge could not save this group label.".to_string())
}

#[derive(Debug, Deserialize)]
pub struct ReplaceClientGroupLabelsRequest {
    pub labels: client_groups::ClientGroupLabels,
}

/// Atomically persists the one-time local migration from raw GUID keys to
/// composite company keys. It never accesses the Tally mirror.
#[tauri::command]
pub fn replace_client_group_labels(
    app: AppHandle,
    request: ReplaceClientGroupLabelsRequest,
) -> Result<(), String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|_| "Bridge could not locate its local group-label configuration.".to_string())?;
    client_groups::replace_labels(&directory, request.labels)
        .map_err(|_| "Bridge could not migrate local group labels.".to_string())
}

/// Saves the optional all-client sort preference without accessing the Tally mirror.
#[tauri::command]
pub fn save_client_sort_preference(
    app: AppHandle,
    preference: client_groups::ClientSortPreference,
) -> Result<(), String> {
    let directory = app.path().app_config_dir().map_err(|_| {
        "Bridge could not locate its local client-preference configuration.".to_string()
    })?;
    client_groups::save_sort_preference(&directory, preference)
        .map_err(|_| "Bridge could not save the all-client sort preference.".to_string())
}
