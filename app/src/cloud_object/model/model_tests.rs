use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cloud_object_client::MockObjectClient;
use lazy_static::lazy_static;
use mockall::Sequence;
use rand::Rng;
use settings::{RespectUserSyncSetting, SyncToCloud};
use warpui::{App, ModelHandle};

use super::*;
use crate::auth::auth_manager::AuthManager;
use crate::auth::user::TEST_USER_UID;
use crate::auth::{AuthStateProvider, UserUid};
use crate::cloud_object::model::actions::ObjectActions;
use crate::cloud_object::model::generic_string_model::GenericStringModel;
use crate::cloud_object::model::view::{CloudViewModel, UpdateTimestamp};
use crate::cloud_object::{
    CloudObjectMetadata, CloudObjectPermissions, CloudObjectStatuses, CloudObjectSyncStatus,
    GenericStringObjectFormat, JsonObjectType, NumInFlightRequests, ObjectIdType, Owner,
    ServerMetadata, ServerObject, ServerPermissions, ServerPreference,
};
use crate::drive::DriveIndexVariant;
use crate::drive::folders::{CloudFolderModel, FolderId};
use crate::features::FeatureFlag;
use crate::server::cloud_objects::listener::ObjectUpdateMessage;
use crate::server::cloud_objects::update_manager::InitialLoadResponse;
use crate::server::ids::{ServerId, ServerIdAndType};
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::object::ObjectClient;
use crate::server::sync_queue::SyncQueue;
use crate::server::telemetry::context_provider::AppTelemetryContextProvider;
use crate::settings::{Preference, init_and_register_user_preferences};
use crate::system::SystemStats;
use crate::workspaces::team::Team;
use crate::workspaces::team_tester::TeamTesterStatus;
use crate::workspaces::user_profiles::UserProfiles;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::{Workspace, WorkspaceUid};
use crate::{NetworkStatus, UpdateManager};

fn create_cloud_model(
    app: &mut App,
    objects: Vec<Box<dyn CloudObject>>,
) -> ModelHandle<CloudModel> {
    // Make sure to register the CloudModel singleton - some CloudObject methods
    // find it and other dependencies via the AppContext.
    app.add_singleton_model(|_ctx| CloudModel::new(None, objects, None))
}

lazy_static! {
    /// Mock the user being on _a_ team in tests, so that the team drive is available.
    /// Otherwise, any team objects will appear shared.
    static ref TEST_TEAM: Team = Team::from_local_cache(
        ServerId::from(1),
        "Test Team".to_string(),
        None,
        None,
        None,
    );

    static ref TEST_WORKSPACE: Workspace = Workspace::from_local_cache(
        WorkspaceUid::from(ServerId::from(1)),
        "Test Workspace".to_string(),
        Some(vec![TEST_TEAM.clone()]),
    );
}

fn initialize_app(
    app: &mut App,
    cached_objects: Vec<Box<dyn CloudObject>>,
    cloud_object_server_api_mock: Arc<impl ObjectClient>,
) {
    // Add the necessary singleton models to the App
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AppTelemetryContextProvider::new_context_provider);
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![TEST_WORKSPACE.clone()], ctx));
    app.add_singleton_model(TeamTesterStatus::new);
    app.add_singleton_model(SyncQueue::mock);
    app.add_singleton_model(|_ctx| CloudModel::new(None, cached_objects, None));
    app.add_singleton_model(|ctx| UpdateManager::new(None, cloud_object_server_api_mock, ctx));
    app.add_singleton_model(|_| UserProfiles::new(Vec::new()));
    app.add_singleton_model(CloudViewModel::new);
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));

    // The start of polling is normally triggered by authentication completion, but
    // we need to do it manually for tests.
    TeamTesterStatus::handle(app).update(app, |team_tester, ctx| {
        team_tester.initiate_data_pollers(false, ctx);
    });
}

fn mock_server_metadata() -> ServerMetadata {
    ServerMetadata {
        uid: ServerId::default(),
        revision: Revision::now(),
        metadata_last_updated_ts: Utc::now().into(),
        trashed_ts: None,
        folder_id: None,
        is_welcome_object: false,
        creator_uid: None,
        last_editor_uid: None,
        current_editor_uid: None,
    }
}

fn mock_server_permissions(owner: Owner) -> ServerPermissions {
    ServerPermissions {
        space: owner,
        guests: Vec::new(),
        permissions_last_updated_ts: Utc::now().into(),
        anyone_link_sharing: None,
    }
}

fn mock_permissions() -> CloudObjectPermissions {
    CloudObjectPermissions {
        owner: Owner::mock_current_user(),
        guests: Vec::new(),
        permissions_last_updated_ts: None,
        anyone_with_link: None,
    }
}

fn mock_random_folders(start_id: i64, owner: Owner) -> Vec<ServerFolder> {
    let mut rng = rand::thread_rng();
    // pick how many folders to generate at random
    let number_of_folders = rng.gen_range(1..10);
    mock_server_folders(start_id, owner, number_of_folders)
}

fn mock_server_folders(start_id: i64, owner: Owner, number_of_folders: i64) -> Vec<ServerFolder> {
    (0..number_of_folders)
        .map(|idx| {
            ServerFolder::new(
                SyncId::ServerId((start_id + idx).into()),
                CloudFolderModel::new(&format!("f{}", start_id + idx), false),
                mock_server_metadata(),
                mock_server_permissions(owner),
            )
        })
        .collect()
}

fn mock_server_preferences() -> Vec<ServerPreference> {
    let owner = Owner::mock_current_user();
    (1_i64..=4)
        .map(|id| {
            ServerPreference::new(
                SyncId::ServerId(id.into()),
                CloudPreferenceModel::new(
                    Preference::new(
                        format!("pref{id}"),
                        "true",
                        SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                    )
                    .expect("error creating preference"),
                ),
                mock_server_metadata(),
                mock_server_permissions(owner),
            )
        })
        .collect()
}

fn mock_cloud_folder(id: SyncId, name: String, folder_id: Option<SyncId>) -> CloudFolder {
    CloudFolder::new(
        id,
        CloudFolderModel {
            name,
            is_open: true,
            is_warp_pack: false,
        },
        CloudObjectMetadata {
            pending_changes_statuses: CloudObjectStatuses {
                content_sync_status: CloudObjectSyncStatus::NoLocalChanges,
                has_pending_metadata_change: false,
                has_pending_permissions_change: false,
                pending_untrash: false,
                pending_delete: false,
            },
            folder_id,
            revision: Default::default(),
            metadata_last_updated_ts: Default::default(),
            current_editor_uid: Default::default(),
            trashed_ts: Default::default(),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
        },
        mock_permissions(),
    )
}

fn mock_trashed_cloud_folder(id: SyncId, name: String, folder_id: Option<SyncId>) -> CloudFolder {
    let mut folder = mock_cloud_folder(id, name, folder_id);
    folder.metadata.trashed_ts = Some(ServerTimestamp::from_unix_timestamp_micros(10).unwrap());
    folder
}

fn folder_from_cloud_model(model: &CloudModel, id: SyncId) -> &CloudFolder {
    model.get_folder_by_uid(&id.uid()).expect("is a folder")
}

/// Mock receiving an RTC update. These tests update objects by mocking RTC messages so that they
/// don't need to mock the server API for updates. The unit tests for [`UpdateManager`] ensure that
/// updates from both RTC and client actions emit the same events.
fn receive_rtc_update(message: ObjectUpdateMessage, app: &mut App) {
    UpdateManager::handle(app).update(app, |update_manager, ctx| {
        update_manager.received_message_from_server(message, ctx)
    });
}

fn move_object(id: ServerId, folder_id: Option<FolderId>, app: &mut App) {
    let message = CloudModel::handle(app).read(app, |cloud_model, _| {
        let object = cloud_model
            .get_by_uid(&id.uid())
            .expect("Expected object to exist in cloud model");

        let metadata = ServerMetadata {
            uid: id,
            revision: object.metadata().revision.expect("Revision is required"),
            current_editor_uid: object.metadata().current_editor_uid.clone(),
            metadata_last_updated_ts: (object
                .metadata()
                .metadata_last_updated_ts
                .expect("Metadata TS is required")
                .utc()
                + chrono::Duration::seconds(1))
            .into(),
            trashed_ts: object.metadata().trashed_ts,
            folder_id,
            is_welcome_object: object.metadata().is_welcome_object,
            creator_uid: object.metadata().creator_uid.clone(),
            last_editor_uid: object.metadata().last_editor_uid.clone(),
        };

        ObjectUpdateMessage::ObjectMetadataChanged { metadata }
    });

    receive_rtc_update(message, app);
}

#[test]
fn test_update_with_deleted_objects() {
    let folders = mock_server_folders(
        5,
        Owner::Team {
            team_uid: ServerId::from(1),
        },
        3,
    );
    let preferences = mock_server_preferences();

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(
            &mut app,
            folders
                .iter()
                .map(|folder| CloudFolder::new_from_server(folder.clone()))
                .map(|o| Box::new(o) as Box<dyn CloudObject>)
                .collect(),
        );
        cloud_model.update(&mut app, |model, ctx| {
            for preference in preferences.clone() {
                model.upsert_from_server_object(preference, ctx);
            }
        });

        // Validate there's some preferences and folders in memory
        cloud_model.read(&app, |cloud_model, _| {
            assert_eq!(3, cloud_model.get_all_active_and_inactive_folders().count());
            assert_eq!(
                4,
                cloud_model
                    .get_all_objects_of_type::<GenericStringObjectId, CloudPreferenceModel>()
                    .count()
            );
            assert_eq!(7, cloud_model.as_cloud_objects().count());
        });

        // Apply the "update from server"
        cloud_model.update(&mut app, |cloud_model, ctx| {
            // Set 3rd preference to have pending changes. This should keep it in memory,
            // even though it's not returned from the server.
            let preference_id: SyncId = SyncId::ServerId(3.into());
            if let Some(object) = cloud_model
                .get_object_of_type_mut::<GenericStringObjectId, CloudPreferenceModel>(
                    &preference_id,
                )
            {
                object.set_pending_content_changes_status(CloudObjectSyncStatus::InFlight(
                    NumInFlightRequests(1),
                ));
            }
            cloud_model.update_objects(preferences.into_iter().take(2), ctx);
            cloud_model.update_objects(folders.into_iter().take(2), ctx);
        });

        cloud_model.read(&app, |cloud_model, _| {
            // expected: 3rd folder was removed on the server, and so we don't want it in
            // memory
            assert_eq!(2, cloud_model.get_all_active_and_inactive_folders().count());
            // expected: 3rd preference has local changes, so we want to keep it, but 4th
            // doesn't and also wasn't returned from the server, so we want to remove it.
            assert_eq!(
                3,
                cloud_model
                    .get_all_objects_of_type::<GenericStringObjectId, CloudPreferenceModel>()
                    .count()
            );
            assert_eq!(5, cloud_model.as_cloud_objects().count());
        });
    })
}

#[test]
fn test_create_json_object() {
    let client_id = ClientId::default();
    let id = SyncId::ClientId(client_id);
    let json_object: Box<dyn CloudObject> = Box::new(CloudPreference::new(
        id,
        GenericStringModel::new(
            Preference::new(
                "test_storage_key".to_owned(),
                "{\"test_key\": \"test_value\"}",
                SyncToCloud::Globally(RespectUserSyncSetting::Yes),
            )
            .expect("error creating preference"),
        ),
        CloudObjectMetadata {
            pending_changes_statuses: CloudObjectStatuses {
                content_sync_status: CloudObjectSyncStatus::NoLocalChanges,
                has_pending_metadata_change: false,
                has_pending_permissions_change: false,
                pending_untrash: false,
                pending_delete: false,
            },
            folder_id: Default::default(),
            revision: Default::default(),
            metadata_last_updated_ts: Default::default(),
            current_editor_uid: Default::default(),
            trashed_ts: Default::default(),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
        },
        mock_permissions(),
    ));

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, vec![json_object]);
        cloud_model.read(&app, |model, _| {
            let json_object: &CloudPreference =
                model.get_object_of_type(&id).expect("model should exist");
            assert_eq!(
                json_object.model().string_model.storage_key,
                "test_storage_key".to_owned()
            );
        });
    })
}

#[test]
fn test_update_object_server_id_for_folder() {
    let client_id = ClientId::new();
    let server_id: FolderId = 1.into();
    let folders: Vec<Box<dyn CloudObject>> = vec![Box::new(CloudFolder::new(
        SyncId::ClientId(client_id),
        CloudFolderModel::new("test", false),
        CloudObjectMetadata {
            pending_changes_statuses: CloudObjectStatuses {
                content_sync_status: CloudObjectSyncStatus::NoLocalChanges,
                has_pending_metadata_change: false,
                has_pending_permissions_change: false,
                pending_untrash: false,
                pending_delete: false,
            },
            folder_id: Default::default(),
            revision: Default::default(),
            metadata_last_updated_ts: Default::default(),
            current_editor_uid: Default::default(),
            trashed_ts: Default::default(),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
        },
        mock_permissions(),
    ))];
    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, folders);
        cloud_model.update(&mut app, |model, ctx| {
            model.update_object_after_server_creation(
                client_id,
                ServerCreationInfo {
                    creator_uid: None,
                    permissions: ServerPermissions::mock_personal(),
                    server_id_and_type: ServerIdAndType {
                        id: server_id.to_server_id(),
                        id_type: ObjectIdType::Folder,
                    },
                },
                ctx,
            )
        });

        cloud_model.read(&app, |model, _| {
            let folder = model
                .get_folder_by_uid(&SyncId::ServerId(server_id.into()).uid())
                .unwrap();
            assert_eq!(folder.id, SyncId::ServerId(server_id.into()));
        });
    })
}

fn base_mock_cloud_object_server_api() -> MockObjectClient {
    MockObjectClient::new()
}

fn check_cloud_folders(app: &mut App, number_of_folders: usize) {
    CloudModel::handle(app).read(app, |model, _| {
        assert_eq!(
            number_of_folders,
            model.get_all_active_and_inactive_folders().count(),
            "we expected {} folders, and received {}",
            number_of_folders,
            model.get_all_active_and_inactive_folders().count()
        );
    });
}

fn check_cloud_preferences(app: &mut App, number_of_preferences: usize) {
    CloudModel::handle(app).read(app, |model, _| {
        assert_eq!(
            number_of_preferences,
            model
                .get_all_objects_of_type::<GenericStringObjectId, CloudPreferenceModel>()
                .count(),
            "we expected {} preferences, and received {}",
            number_of_preferences,
            model
                .get_all_objects_of_type::<GenericStringObjectId, CloudPreferenceModel>()
                .count()
        );
    });
}

#[test]
/// This test validates the behavior when the CloudModel has been initiated with empty values (ie.
/// empty sqlite / local cache). We check that after initialization, we fetch the values and
/// successfully update CloudModel.
// TODO followup with a test equivalent for when the user is not team tester but still has teams?
fn test_load_cloud_objects_on_initial_load_with_empty_cache() {
    let _flag = FeatureFlag::KnowledgeSidebar.override_enabled(true);

    let personal_folders = mock_random_folders(300, Owner::mock_current_user());
    let personal_folders_len = personal_folders.len();
    let team_folders = mock_random_folders(
        400,
        Owner::Team {
            team_uid: ServerId::from(1),
        },
    );
    let team_folders_len = team_folders.len();
    let preferences = mock_server_preferences();
    let preferences_len = preferences.len();
    let all_folders = [personal_folders, team_folders].concat();

    App::test((), |mut app| async move {
        // Setup the app and APIs
        let mut cloud_object_server_api_mock = base_mock_cloud_object_server_api();
        cloud_object_server_api_mock
            .expect_fetch_changed_objects()
            .times(1)
            // since we don't have anything cached, at startup we just send empty list here
            .withf(|objects_to_update, _| {
                objects_to_update.folders.is_empty()
                    && objects_to_update.generic_string_objects.is_empty()
            })
            .return_once(move |_, _| {
                Ok(InitialLoadResponse {
                    updated_folders: all_folders,
                    deleted_folders: vec![],
                    user_profiles: vec![],
                    updated_generic_string_objects: HashMap::from([(
                        GenericStringObjectFormat::Json(JsonObjectType::Preference),
                        preferences
                            .into_iter()
                            .map(|object| Box::new(object) as Box<dyn ServerObject>)
                            .collect(),
                    )]),
                    deleted_generic_string_objects: Default::default(),
                    action_histories: Default::default(),
                })
            });

        // No folders or preferences (or other objects) loaded from sqlite passed to CloudModel
        initialize_app(&mut app, Vec::new(), Arc::new(cloud_object_server_api_mock));

        // Spend time waiting for the initial load to finish etc.
        warpui::r#async::Timer::after(Duration::from_secs(1)).await;

        // TODO: @ianhodge - update tests once cloud model APIs are added
        // Now CloudModel should include all objects that were fetched via initial load
        check_cloud_folders(&mut app, personal_folders_len + team_folders_len);
        check_cloud_preferences(&mut app, preferences_len);
    })
}

#[test]
fn test_loading_all_cloud_objects_after_switching_from_offline() {
    let _flag = FeatureFlag::KnowledgeSidebar.override_enabled(true);

    let personal_folders = mock_random_folders(300, Owner::mock_current_user());
    let personal_folders_len = personal_folders.len();
    let team_folders = mock_random_folders(
        400,
        Owner::Team {
            team_uid: ServerId::from(1),
        },
    );
    let team_folders_len = team_folders.len();
    let preferences = mock_server_preferences();
    let preferences_len = preferences.len();

    App::test((), |mut app| async move {
        // Sequences used for ordering requests (so first call will return something different than
        // next etc.)
        let mut cloud_objects_sequence = Sequence::new();

        // Setup the app and APIs
        let mut cloud_object_server_api_mock = base_mock_cloud_object_server_api();

        // Update manager also calls for update based on the current in memory state
        // We only expect it once with the given set of arguments (empty vector) and it'll return
        // personal folders
        cloud_object_server_api_mock
            .expect_fetch_changed_objects()
            .times(1)
            // since we don't have anything cached, at startup we just send empty list here
            .withf(|objects_to_update, _| {
                objects_to_update.folders.is_empty()
                    && objects_to_update.generic_string_objects.is_empty()
            })
            .in_sequence(&mut cloud_objects_sequence)
            .return_once(move |_, _| {
                Ok(InitialLoadResponse {
                    updated_folders: personal_folders,
                    deleted_folders: vec![],
                    user_profiles: vec![],
                    updated_generic_string_objects: Default::default(),
                    deleted_generic_string_objects: Default::default(),
                    action_histories: Default::default(),
                })
            });

        // Second call will return objects from the team (as a list of updated / new to user objects)
        // It'll also be called only once, and with personal_folders in the input
        // We expect this call to happen _after_ the network status change (offline -> online).
        cloud_object_server_api_mock
            .expect_fetch_changed_objects()
            .times(1)
            // verify that the list of objects passed equals the number of personal folders we
            // already have
            .withf(move |objects_to_update, _| {
                objects_to_update.folders.len() == personal_folders_len
            })
            .in_sequence(&mut cloud_objects_sequence)
            .returning(move |_, _| {
                Ok(InitialLoadResponse {
                    updated_folders: team_folders.clone(),
                    deleted_folders: vec![],
                    user_profiles: vec![],
                    updated_generic_string_objects: HashMap::from([(
                        GenericStringObjectFormat::Json(JsonObjectType::Preference),
                        preferences
                            .iter()
                            .cloned()
                            .map(|object| Box::new(object) as Box<dyn ServerObject>)
                            .collect(),
                    )]),
                    deleted_generic_string_objects: Default::default(),
                    action_histories: Default::default(),
                })
            });

        // No folders or preferences (or other objects) loaded from sqlite passed to CloudModel
        initialize_app(&mut app, Vec::new(), Arc::new(cloud_object_server_api_mock));
        check_cloud_preferences(&mut app, 0);
        check_cloud_folders(&mut app, 0);

        // Spend time waiting for the initial load to finish etc.
        warpui::r#async::Timer::after(Duration::from_secs(1)).await;

        // Now CloudModel should include all objects that were fetched via initial load (in this
        // case: personal only)
        check_cloud_folders(&mut app, personal_folders_len);
        check_cloud_preferences(&mut app, 0);

        // Lets go offline
        NetworkStatus::handle(&app).update(&mut app, |network_status, ctx| {
            network_status.reachability_changed(false, ctx);
        });

        // Lets go back online
        NetworkStatus::handle(&app).update(&mut app, |network_status, ctx| {
            network_status.reachability_changed(true, ctx);
        });

        // Spend time waiting for the load to finish etc.
        warpui::r#async::Timer::after(Duration::from_secs(1)).await;

        // Now CloudModel should include all objects that were fetched via initial load
        check_cloud_folders(&mut app, personal_folders_len + team_folders_len);
        check_cloud_preferences(&mut app, preferences_len);
    })
}

#[test]
fn test_force_refresh_only_happens_once() {
    let _flag = FeatureFlag::KnowledgeSidebar.override_enabled(true);

    App::test((), |mut app| async move {
        let mut cloud_objects_sequence = Sequence::new();
        let mut cloud_object_server_api_mock = base_mock_cloud_object_server_api();

        // We expect force_refresh to be true on this request.
        cloud_object_server_api_mock
            .expect_fetch_changed_objects()
            .times(1)
            .withf(|objects_to_update, force_refresh| {
                objects_to_update.folders.is_empty()
                    && objects_to_update.generic_string_objects.is_empty()
                    && *force_refresh
            })
            .in_sequence(&mut cloud_objects_sequence)
            .return_once(move |_, _| {
                Ok(InitialLoadResponse {
                    updated_folders: vec![],
                    deleted_folders: vec![],
                    user_profiles: vec![],
                    updated_generic_string_objects: Default::default(),
                    deleted_generic_string_objects: Default::default(),
                    action_histories: Default::default(),
                })
            });

        // Initialize app with pending refresh = true!
        initialize_app(&mut app, Vec::new(), Arc::new(cloud_object_server_api_mock));

        // Spend time waiting for the initial load to finish etc.
        warpui::r#async::Timer::after(Duration::from_secs(1)).await;

        // Check that pending refresh is now false on CloudModel
        CloudModel::handle(&app).read(&app, |model, _ctx| {
            assert!(!model.cloud_objects_force_refresh_pending())
        });
    })
}

#[test]
fn test_force_refresh_correctly_resets_timestamp() {
    let _flag = FeatureFlag::KnowledgeSidebar.override_enabled(true);

    App::test((), |mut app| async move {
        let mut cloud_objects_sequence = Sequence::new();
        let mut cloud_object_server_api_mock = base_mock_cloud_object_server_api();

        // We expect force_refresh to be true on this request.
        cloud_object_server_api_mock
            .expect_fetch_changed_objects()
            .times(1)
            .withf(|objects_to_update, force_refresh| {
                objects_to_update.folders.is_empty()
                    && objects_to_update.generic_string_objects.is_empty()
                    && *force_refresh
            })
            .in_sequence(&mut cloud_objects_sequence)
            .return_once(move |_, _| {
                Ok(InitialLoadResponse {
                    updated_folders: vec![],
                    deleted_folders: vec![],
                    user_profiles: vec![],
                    updated_generic_string_objects: Default::default(),
                    deleted_generic_string_objects: Default::default(),
                    action_histories: Default::default(),
                })
            });

        // Initialize app with pending refresh = true!
        let start = Utc::now();
        initialize_app(&mut app, Vec::new(), Arc::new(cloud_object_server_api_mock));

        // Spend time waiting for the initial load to finish etc.
        warpui::r#async::Timer::after(Duration::from_secs(1)).await;

        // Check that pending refresh is within the acceptable hour range. The lower bound is
        // measured from `start`: the refresh is scheduled relative to the clock at the moment it
        // completes, which is before this check runs, so comparing against `Utc::now()` here
        // fails whenever the random offset lands on the minimum.
        CloudModel::handle(&app).read(&app, |model, _ctx| {
            let time_option = model.time_of_next_force_refresh;
            assert!(time_option.is_some());
            let time = time_option.unwrap();
            assert!(
                time <= (Utc::now()
                    + chrono::Duration::minutes(MAX_MINUTES_UNTIL_NEXT_FORCE_REFRESH))
            );
            assert!(
                time >= (start + chrono::Duration::minutes(MIN_MINUTES_UNTIL_NEXT_FORCE_REFRESH))
            );
        });
    })
}

#[test]
fn test_collapse_all_in_location() {
    /*
       the folder structure looks like:

       test1
        ↳ test 4
         ↳ test 5
       test 2
        ↳ test 6
         ↳ test 7
       test 3

    */
    let folder_1_id: SyncId = SyncId::ServerId(1.into());
    let folder_2_id: SyncId = SyncId::ServerId(2.into());
    let folder_3_id: SyncId = SyncId::ServerId(3.into());
    let folder_4_id: SyncId = SyncId::ServerId(4.into());
    let folder_5_id: SyncId = SyncId::ServerId(5.into());
    let folder_6_id: SyncId = SyncId::ServerId(6.into());
    let folder_7_id: SyncId = SyncId::ServerId(7.into());

    let folders = vec![
        mock_cloud_folder(folder_1_id, "test1".to_string(), None),
        mock_cloud_folder(folder_2_id, "test2".to_string(), None),
        mock_cloud_folder(folder_3_id, "test3".to_string(), None),
        mock_cloud_folder(folder_4_id, "test4".to_string(), Some(folder_1_id)),
        mock_cloud_folder(folder_5_id, "test5".to_string(), Some(folder_4_id)),
        mock_cloud_folder(folder_6_id, "test6".to_string(), Some(folder_2_id)),
        mock_cloud_folder(folder_7_id, "test7".to_string(), Some(folder_6_id)),
    ]
    .into_iter()
    .map(|o| Box::new(o) as Box<dyn CloudObject>)
    .collect();

    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let cloud_model = create_cloud_model(&mut app, folders);

        cloud_model.update(&mut app, |model, ctx| {
            // first, collapse all folders in folder 1
            model.collapse_all_in_location(
                CloudObjectLocation::Folder(folder_1_id),
                DriveIndexVariant::MainIndex,
                ctx,
            );

            // folders 1, 4, and 5 should be collapsed
            let folder_1 = folder_from_cloud_model(model, folder_1_id);
            let folder_4 = folder_from_cloud_model(model, folder_4_id);
            let folder_5 = folder_from_cloud_model(model, folder_5_id);
            assert!(!folder_1.model().is_open);
            assert!(!folder_4.model().is_open);
            assert!(!folder_5.model().is_open);
            // but the others are still open
            let folder_2 = folder_from_cloud_model(model, folder_2_id);
            let folder_3 = folder_from_cloud_model(model, folder_3_id);
            let folder_6 = folder_from_cloud_model(model, folder_6_id);
            let folder_7 = folder_from_cloud_model(model, folder_7_id);
            assert!(folder_2.model().is_open);
            assert!(folder_3.model().is_open);
            assert!(folder_6.model().is_open);
            assert!(folder_7.model().is_open);

            model.collapse_all_in_location(
                CloudObjectLocation::Space(Default::default()),
                DriveIndexVariant::MainIndex,
                ctx,
            );
            // now all folders in this space are collapsed
            let folder_1 = folder_from_cloud_model(model, folder_1_id);
            let folder_2 = folder_from_cloud_model(model, folder_2_id);
            let folder_3 = folder_from_cloud_model(model, folder_3_id);
            let folder_4 = folder_from_cloud_model(model, folder_4_id);
            let folder_5 = folder_from_cloud_model(model, folder_5_id);
            let folder_6 = folder_from_cloud_model(model, folder_6_id);
            let folder_7 = folder_from_cloud_model(model, folder_7_id);
            assert!(!folder_1.model().is_open);
            assert!(!folder_2.model().is_open);
            assert!(!folder_3.model().is_open);
            assert!(!folder_4.model().is_open);
            assert!(!folder_5.model().is_open);
            assert!(!folder_6.model().is_open);
            assert!(!folder_7.model().is_open);
        });
    })
}

#[test]
fn test_collapse_all_in_trash() {
    /*
       the folder structure looks like:

       test1 -- trashed by user
        ↳ test 4
         ↳ test 5 -- trashed by user
       test 2 -- trashed by user
        ↳ test 6
         ↳ test 7
       test 3 -- trashed by user

       the structure in the trash index looks like:

       test1 -- trashed by user
        ↳ test 4
       test 5 -- trashed by user
       test 2 -- trashed by user
        ↳ test 6
         ↳ test 7
       test 3 -- trashed by user

    */
    let folder_1_id: SyncId = SyncId::ServerId(1.into());
    let folder_2_id: SyncId = SyncId::ServerId(2.into());
    let folder_3_id: SyncId = SyncId::ServerId(3.into());
    let folder_4_id: SyncId = SyncId::ServerId(4.into());
    let folder_5_id: SyncId = SyncId::ServerId(5.into());
    let folder_6_id: SyncId = SyncId::ServerId(6.into());
    let folder_7_id: SyncId = SyncId::ServerId(7.into());

    let folders = vec![
        mock_trashed_cloud_folder(folder_1_id, "test1".to_string(), None),
        mock_trashed_cloud_folder(folder_2_id, "test2".to_string(), None),
        mock_trashed_cloud_folder(folder_3_id, "test3".to_string(), None),
        mock_cloud_folder(folder_4_id, "test4".to_string(), Some(folder_1_id)),
        mock_trashed_cloud_folder(folder_5_id, "test5".to_string(), Some(folder_4_id)),
        mock_cloud_folder(folder_6_id, "test6".to_string(), Some(folder_2_id)),
        mock_cloud_folder(folder_7_id, "test7".to_string(), Some(folder_6_id)),
    ]
    .into_iter()
    .map(|o| Box::new(o) as Box<dyn CloudObject>)
    .collect();

    App::test((), |mut app| async move {
        app.add_singleton_model(UserWorkspaces::default_mock);
        let cloud_model = create_cloud_model(&mut app, folders);

        cloud_model.update(&mut app, |model, ctx| {
            // first, collapse all folders in folder 1
            model.collapse_all_in_location(
                CloudObjectLocation::Folder(folder_1_id),
                DriveIndexVariant::Trash,
                ctx,
            );

            // folders 1, 4 should be collapsed
            let folder_1 = folder_from_cloud_model(model, folder_1_id);
            let folder_4 = folder_from_cloud_model(model, folder_4_id);
            assert!(!folder_1.model().is_open);
            assert!(!folder_4.model().is_open);
            // but the others, including folder 5, are still open
            let folder_2 = folder_from_cloud_model(model, folder_2_id);
            let folder_3 = folder_from_cloud_model(model, folder_3_id);
            let folder_5 = folder_from_cloud_model(model, folder_5_id);
            let folder_6 = folder_from_cloud_model(model, folder_6_id);
            let folder_7 = folder_from_cloud_model(model, folder_7_id);
            assert!(folder_2.model().is_open);
            assert!(folder_3.model().is_open);
            assert!(folder_5.model().is_open);
            assert!(folder_6.model().is_open);
            assert!(folder_7.model().is_open);

            model.collapse_all_in_location(
                CloudObjectLocation::Space(Default::default()),
                DriveIndexVariant::Trash,
                ctx,
            );
            // now all folders in this space are collapsed
            let folder_1 = folder_from_cloud_model(model, folder_1_id);
            let folder_2 = folder_from_cloud_model(model, folder_2_id);
            let folder_3 = folder_from_cloud_model(model, folder_3_id);
            let folder_4 = folder_from_cloud_model(model, folder_4_id);
            let folder_5 = folder_from_cloud_model(model, folder_5_id);
            let folder_6 = folder_from_cloud_model(model, folder_6_id);
            let folder_7 = folder_from_cloud_model(model, folder_7_id);
            assert!(!folder_1.model().is_open);
            assert!(!folder_2.model().is_open);
            assert!(!folder_3.model().is_open);
            assert!(!folder_4.model().is_open);
            assert!(!folder_5.model().is_open);
            assert!(!folder_6.model().is_open);
            assert!(!folder_7.model().is_open);
        });
    })
}

#[test]
fn test_breadcrumbs() {
    let folder_1_id: SyncId = SyncId::ServerId(1.into());
    let folder_2_id: SyncId = SyncId::ServerId(2.into());
    let folder_3_id: SyncId = SyncId::ServerId(3.into());

    let folders = vec![
        mock_cloud_folder(folder_1_id, "test1".to_string(), None),
        mock_cloud_folder(folder_2_id, "test2".to_string(), Some(folder_1_id)),
        mock_cloud_folder(folder_3_id, "test3".to_string(), Some(folder_2_id)),
    ]
    .into_iter()
    .map(|f| Box::new(f) as Box<dyn CloudObject>)
    .collect::<Vec<_>>();

    App::test((), |mut app| async move {
        let cloud_object_server_api_mock = base_mock_cloud_object_server_api();
        initialize_app(
            &mut app,
            folders.clone(),
            Arc::new(cloud_object_server_api_mock),
        );

        CloudModel::handle(&app).read(&app, |_, ctx| {
            assert_eq!("Personal".to_string(), folders[0].breadcrumbs(ctx));
            assert_eq!("Personal / test1".to_string(), folders[1].breadcrumbs(ctx));
            assert_eq!(
                "Personal / test1 / test2".to_string(),
                folders[2].breadcrumbs(ctx)
            );
        });
    });
}

/// Asserts that the object with the given ID has the expected sorting timestamp.
#[track_caller]
fn assert_sorting_timestamp(id: ServerId, expected_ts: impl Into<ServerTimestamp>, app: &App) {
    let sorting_timestamp = app.read(|ctx| {
        let object = CloudModel::as_ref(ctx).get_by_uid(&id.uid())?;
        CloudViewModel::as_ref(ctx).object_sorting_timestamp(object, UpdateTimestamp::Revision, ctx)
    });
    assert_eq!(
        sorting_timestamp,
        Some(expected_ts.into()),
        "Unexpected timestamp for {}",
        id.uid()
    );
}

/// Test that, if an object is updated, we recalculate its ancestors' sorting timestamps too. This
/// way, the folders containing the updated object move to the top of the Warp Drive index if it's
/// sorted by last updated.
#[test]
fn test_update_folder_timestamp_from_child_update() {
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let folder_id: ServerId = 123.into();
        let parent_folder_id: ServerId = 456.into();
        let child_folder_id: ServerId = 789.into();
        let initial_ts = Utc::now();

        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            let mut folder = mock_cloud_folder(
                folder_id.into(),
                "Folder".to_string(),
                Some(parent_folder_id.into()),
            );
            folder.metadata.revision = Some(initial_ts.into());

            let mut parent_folder =
                mock_cloud_folder(parent_folder_id.into(), "Parent Folder".to_string(), None);
            parent_folder.metadata.revision = Some(initial_ts.into());

            let mut child_folder = mock_cloud_folder(
                child_folder_id.into(),
                "Test Folder".to_string(),
                Some(folder_id.into()),
            );
            child_folder.metadata.revision = Some(initial_ts.into());

            cloud_model.add_object(folder_id.into(), folder);
            cloud_model.add_object(parent_folder_id.into(), parent_folder);
            cloud_model.add_object(child_folder_id.into(), child_folder);
        });

        // At first, all 3 objects should have the initial sorting timestamp.
        assert_sorting_timestamp(folder_id, initial_ts, &app);
        assert_sorting_timestamp(parent_folder_id, initial_ts, &app);
        assert_sorting_timestamp(child_folder_id, initial_ts, &app);

        // After updating the child folder, all 3 timestamps should change.
        let new_ts = initial_ts + chrono::Duration::seconds(5);
        receive_rtc_update(
            ObjectUpdateMessage::ObjectContentChanged {
                server_object: Box::new(ServerCloudObject::Folder(ServerFolder::new(
                    SyncId::ServerId(child_folder_id),
                    CloudFolderModel::new("Test Folder", false),
                    ServerMetadata {
                        uid: child_folder_id,
                        revision: new_ts.into(),
                        metadata_last_updated_ts: new_ts.into(),
                        trashed_ts: None,
                        folder_id: Some(folder_id.into()),
                        is_welcome_object: false,
                        creator_uid: None,
                        last_editor_uid: None,
                        current_editor_uid: None,
                    },
                    mock_server_permissions(Owner::mock_current_user()),
                ))),
                last_editor: None,
            },
            &mut app,
        );

        assert_sorting_timestamp(folder_id, new_ts, &app);
        assert_sorting_timestamp(parent_folder_id, new_ts, &app);
        assert_sorting_timestamp(child_folder_id, new_ts, &app);
    });
}

/// Tests that, if an object is moved from one folder to another, we recalculate the sorting
/// timestamps of both. If the object was the most-recently-updated in its old folder, the old
/// folder's sorting timestamp will decrease. If it's the most-recently-updated object in the new
/// folder, the new folder's sorting timestamp will increase.
#[test]
fn test_update_folder_timestamp_from_object_move() {
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let folder_a_id: ServerId = 123.into();
        let folder_b_id: ServerId = 456.into();
        let child_folder_id: ServerId = 789.into();

        let t1 = Utc::now();
        let t2 = t1 + chrono::Duration::seconds(5);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            let mut folder_a = mock_cloud_folder(folder_a_id.into(), "Folder A".to_string(), None);
            folder_a.metadata.revision = Some(t1.into());

            let mut folder_b = mock_cloud_folder(folder_b_id.into(), "Folder B".to_string(), None);
            folder_b.metadata.revision = Some(t1.into());

            let mut child_folder = mock_cloud_folder(
                child_folder_id.into(),
                "Test Folder".to_string(),
                Some(folder_a_id.into()),
            );
            child_folder.metadata.revision = Some(t2.into());
            child_folder.metadata.metadata_last_updated_ts = Some(t2.into());

            cloud_model.add_object(folder_a_id.into(), folder_a);
            cloud_model.add_object(folder_b_id.into(), folder_b);
            cloud_model.add_object(child_folder_id.into(), child_folder);
        });

        // At first, folder A and the child folder sort by the child folder's timestamp, and folder B sorts
        // by its older timestamp.
        assert_sorting_timestamp(folder_a_id, t2, &app);
        assert_sorting_timestamp(folder_b_id, t1, &app);
        assert_sorting_timestamp(child_folder_id, t2, &app);

        // Move the child folder to folder B, so it now has the newer sort timestamp.
        move_object(child_folder_id, Some(folder_b_id.into()), &mut app);

        assert_sorting_timestamp(folder_a_id, t1, &app);
        assert_sorting_timestamp(folder_b_id, t2, &app);

        // Move the child folder into the root, so both folders have the older sort timestamp.
        move_object(child_folder_id, None, &mut app);
        assert_sorting_timestamp(folder_a_id, t1, &app);
        assert_sorting_timestamp(folder_b_id, t1, &app);
    });
}

/// Tests that, if an object is created in a folder, we recalculate its ancestors' sorting
/// timestamp. The new object will likely be the most-recently-updated child of the folder, so the
/// folder's sorting timestamp will increase.
#[test]
fn test_update_folder_timestamp_from_new_child() {
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let folder_id: ServerId = 123.into();
        let parent_folder_id: ServerId = 456.into();
        let child_folder_id: ServerId = 789.into();
        let t1 = Utc::now();
        let t2 = t1 + chrono::Duration::seconds(5);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            let mut folder = mock_cloud_folder(
                folder_id.into(),
                "Folder".to_string(),
                Some(parent_folder_id.into()),
            );
            folder.metadata.revision = Some(t1.into());

            let mut parent_folder =
                mock_cloud_folder(parent_folder_id.into(), "Parent Folder".to_string(), None);
            parent_folder.metadata.revision = Some(t1.into());

            cloud_model.add_object(folder_id.into(), folder);
            cloud_model.add_object(parent_folder_id.into(), parent_folder);
        });

        // At first, only the two folders exist.
        assert_sorting_timestamp(folder_id, t1, &app);
        assert_sorting_timestamp(parent_folder_id, t1, &app);

        // Create a child folder inside the folder.
        receive_rtc_update(
            ObjectUpdateMessage::ObjectContentChanged {
                server_object: Box::new(ServerCloudObject::Folder(ServerFolder::new(
                    SyncId::ServerId(child_folder_id),
                    CloudFolderModel::new("Test Folder", false),
                    ServerMetadata {
                        uid: child_folder_id,
                        revision: t2.into(),
                        metadata_last_updated_ts: t2.into(),
                        trashed_ts: None,
                        folder_id: Some(folder_id.into()),
                        is_welcome_object: false,
                        creator_uid: None,
                        last_editor_uid: None,
                        current_editor_uid: None,
                    },
                    mock_server_permissions(Owner::mock_current_user()),
                ))),
                last_editor: None,
            },
            &mut app,
        );

        // The child folder timestamp is now the sort timestamp for the folders.
        assert_sorting_timestamp(folder_id, t2, &app);
        assert_sorting_timestamp(parent_folder_id, t2, &app);
    });
}

/// Tests that, if an object is trashed or untrashed, we recalculate its folder's sorting timestamp.
/// Only untrashed children count towards a folder's sorting timestamp, so trashing/untrashing
/// effectively changes the folder's contents.
#[test]
fn test_update_folder_timestamp_from_child_trash() {
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let child_folder_id: ServerId = 123.into();
        let folder_id: ServerId = 456.into();

        let t1 = Utc::now();
        let t2 = t1 + chrono::Duration::seconds(1);
        let t3 = t2 + chrono::Duration::seconds(1);
        let t4 = t3 + chrono::Duration::seconds(1);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            let mut folder = mock_cloud_folder(folder_id.into(), "Folder".to_string(), None);
            folder.metadata.revision = Some(t1.into());

            let mut child_folder = mock_cloud_folder(
                child_folder_id.into(),
                "Folder".to_string(),
                Some(folder_id.into()),
            );
            child_folder.metadata.revision = Some(t2.into());
            child_folder.metadata.metadata_last_updated_ts = Some(t2.into());

            cloud_model.add_object(folder_id.into(), folder);
            cloud_model.add_object(child_folder_id.into(), child_folder);
        });

        assert_sorting_timestamp(folder_id, t2, &app);

        // Trash the child folder so that it no longer counts towards the folder's sort timestamp.
        receive_rtc_update(
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: ServerMetadata {
                    uid: child_folder_id,
                    revision: t2.into(),
                    metadata_last_updated_ts: t3.into(),
                    trashed_ts: Some(t3.into()),
                    folder_id: Some(folder_id.into()),
                    is_welcome_object: false,
                    creator_uid: None,
                    last_editor_uid: None,
                    current_editor_uid: None,
                },
            },
            &mut app,
        );

        assert_sorting_timestamp(folder_id, t1, &app);

        // Untrash the child folder, updating the folder timestamp.
        receive_rtc_update(
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: ServerMetadata {
                    uid: child_folder_id,
                    revision: t2.into(),
                    metadata_last_updated_ts: t4.into(),
                    trashed_ts: None,
                    folder_id: Some(folder_id.into()),
                    is_welcome_object: false,
                    creator_uid: None,
                    last_editor_uid: None,
                    current_editor_uid: None,
                },
            },
            &mut app,
        );

        assert_sorting_timestamp(folder_id, t2, &app);
    });
}

#[test]
fn test_shared_personal_object() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let other_user = UserUid::new("other_user");
        let shared_child_folder_id = SyncId::ServerId(123.into());
        let shared_child_folder = CloudFolder::new(
            shared_child_folder_id,
            CloudFolderModel::new("Shared Folder", false),
            CloudObjectMetadata::new_from_server(mock_server_metadata()),
            CloudObjectPermissions {
                owner: Owner::User {
                    user_uid: other_user,
                },
                guests: Vec::new(),
                permissions_last_updated_ts: None,
                anyone_with_link: None,
            },
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, ctx| {
            cloud_model.add_object(shared_child_folder_id, shared_child_folder);

            let space = cloud_model
                .get_folder(&shared_child_folder_id)
                .expect("Folder is in CloudModel")
                .space(ctx);
            assert_eq!(space, Space::Shared);
        });
    });
}

#[test]
fn test_unshared_personal_object() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let shared_child_folder_id = SyncId::ServerId(123.into());
        let shared_child_folder = CloudFolder::new(
            shared_child_folder_id,
            CloudFolderModel::new("Shared Folder", false),
            CloudObjectMetadata::new_from_server(mock_server_metadata()),
            CloudObjectPermissions {
                owner: Owner::User {
                    user_uid: UserUid::new(TEST_USER_UID),
                },
                guests: Vec::new(),
                permissions_last_updated_ts: None,
                anyone_with_link: None,
            },
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, ctx| {
            cloud_model.add_object(shared_child_folder_id, shared_child_folder);

            let space = cloud_model
                .get_folder(&shared_child_folder_id)
                .expect("Folder is in CloudModel")
                .space(ctx);
            assert_eq!(space, Space::Personal);
        });
    });
}

#[test]
fn test_shared_team_object() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        // The user is not on this team.
        let team_uid = ServerId::from(456);

        let shared_child_folder_id = SyncId::ServerId(123.into());
        let shared_child_folder = CloudFolder::new(
            shared_child_folder_id,
            CloudFolderModel::new("Shared Folder", false),
            CloudObjectMetadata::new_from_server(mock_server_metadata()),
            CloudObjectPermissions {
                owner: Owner::Team { team_uid },
                guests: Vec::new(),
                permissions_last_updated_ts: None,
                anyone_with_link: None,
            },
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, ctx| {
            cloud_model.add_object(shared_child_folder_id, shared_child_folder);

            let space = cloud_model
                .get_folder(&shared_child_folder_id)
                .expect("Folder is in CloudModel")
                .space(ctx);
            assert_eq!(space, Space::Shared);
        });
    });
}

#[test]
fn test_unshared_team_object() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test((), |mut app| async move {
        app.update(init_and_register_user_preferences);
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        // Use the current user's team.
        let team_uid = TEST_TEAM.uid;
        let shared_child_folder_id = SyncId::ServerId(123.into());
        let shared_child_folder = CloudFolder::new(
            shared_child_folder_id,
            CloudFolderModel::new("Shared Folder", false),
            CloudObjectMetadata::new_from_server(mock_server_metadata()),
            CloudObjectPermissions {
                owner: Owner::Team { team_uid },
                guests: Vec::new(),
                permissions_last_updated_ts: None,
                anyone_with_link: None,
            },
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, ctx| {
            cloud_model.add_object(shared_child_folder_id, shared_child_folder);

            let space = cloud_model
                .get_folder(&shared_child_folder_id)
                .expect("Folder is in CloudModel")
                .space(ctx);
            assert_eq!(space, Space::Team { team_uid });
        });
    });
}

#[test]
fn test_shared_object_in_unshared_folder() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test((), |mut app| async move {
        app.update(init_and_register_user_preferences);
        initialize_app(
            &mut app,
            Vec::new(),
            Arc::new(base_mock_cloud_object_server_api()),
        );

        let other_user = UserUid::new("other_user");
        let unshared_folder_id = SyncId::ServerId(567.into());
        let shared_child_folder_id = SyncId::ServerId(123.into());
        let mut shared_child_folder = CloudFolder::new(
            shared_child_folder_id,
            CloudFolderModel::new("Shared Folder", false),
            CloudObjectMetadata::new_from_server(mock_server_metadata()),
            CloudObjectPermissions {
                owner: Owner::User {
                    user_uid: other_user,
                },
                guests: Vec::new(),
                permissions_last_updated_ts: None,
                anyone_with_link: None,
            },
        );
        shared_child_folder.metadata_mut().folder_id = Some(unshared_folder_id);

        CloudModel::handle(&app).update(&mut app, |cloud_model, ctx| {
            cloud_model.add_object(shared_child_folder_id, shared_child_folder);
            let child_folder = cloud_model
                .get_folder(&shared_child_folder_id)
                .expect("Folder is in CloudModel");

            // Check space-based APIs.
            assert_eq!(child_folder.space(ctx), Space::Shared);
            assert!(child_folder.is_in_space(Space::Shared, ctx));

            // Check location-based APIs.
            assert_eq!(
                child_folder.location(cloud_model, ctx),
                CloudObjectLocation::Space(Space::Shared)
            );
            assert!(child_folder.metadata.folder_id.is_some());

            // Despite the missing parent folder, the child folder is not trashed.
            assert!(!child_folder.is_trashed(cloud_model));

            // Check that iteration APIs include the child folder where it's expected.
            assert!(
                cloud_model
                    .active_cloud_objects_in_space(Space::Shared, ctx)
                    .any(|obj| obj.uid() == child_folder.uid())
            );
            assert!(
                cloud_model
                    .active_cloud_objects_in_location_without_descendents(
                        CloudObjectLocation::Space(Space::Shared),
                        ctx
                    )
                    .any(|obj| obj.uid() == child_folder.uid())
            );
            assert_eq!(
                cloud_model
                    .trashed_cloud_objects_in_space(Space::Shared, ctx)
                    .count(),
                0
            );
            assert_eq!(
                cloud_model
                    .trashed_cloud_objects_in_location_without_descendents(
                        CloudObjectLocation::Space(Space::Shared),
                        ctx
                    )
                    .count(),
                0
            );

            let folder_location = CloudObjectLocation::Folder(unshared_folder_id);
            assert_eq!(
                cloud_model
                    .active_cloud_objects_in_location_without_descendents(folder_location, ctx)
                    .count(),
                0
            );
            assert_eq!(
                cloud_model
                    .trashed_cloud_objects_in_location_without_descendents(folder_location, ctx)
                    .count(),
                0
            );
        });
    });
}

/// Helper: compute active UIDs using the naive (non-memoized) is_trashed approach.
fn naive_active_object_uids(model: &CloudModel) -> HashSet<String> {
    model
        .as_cloud_objects()
        .filter(|obj| !obj.is_trashed(model))
        .map(|obj| obj.uid())
        .collect()
}

#[test]
fn active_object_uids_matches_naive_with_no_trashed_objects() {
    let folder_id = SyncId::ServerId(1.into());
    let objects: Vec<Box<dyn CloudObject>> = vec![
        Box::new(mock_cloud_folder(folder_id, "Folder".into(), None)),
        Box::new(mock_cloud_folder(
            SyncId::ServerId(2.into()),
            "Folder".into(),
            Some(folder_id),
        )),
    ];

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, objects);
        cloud_model.read(&app, |model, _| {
            assert_eq!(model.active_object_uids(), naive_active_object_uids(model));
            assert_eq!(model.active_object_uids().len(), 2);
        });
    });
}

#[test]
fn active_object_uids_matches_naive_with_directly_trashed_object() {
    let trashed_folder_id = SyncId::ServerId(1.into());
    let active_folder_id = SyncId::ServerId(2.into());
    let objects: Vec<Box<dyn CloudObject>> = vec![
        Box::new(mock_trashed_cloud_folder(
            trashed_folder_id,
            "Trashed Folder".into(),
            None,
        )),
        Box::new(mock_cloud_folder(
            active_folder_id,
            "Active Folder".into(),
            None,
        )),
    ];

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, objects);
        cloud_model.read(&app, |model, _| {
            let active = model.active_object_uids();
            assert_eq!(active, naive_active_object_uids(model));
            assert_eq!(active.len(), 1);
            assert!(active.contains(&active_folder_id.uid()));
            assert!(!active.contains(&trashed_folder_id.uid()));
        });
    });
}

#[test]
fn active_object_uids_matches_naive_with_indirectly_trashed_children() {
    // A trashed folder with a non-trashed child folder inside it.
    // The child folder should be considered trashed (indirectly) by both approaches.
    let trashed_folder_id = SyncId::ServerId(1.into());
    let child_folder_id = SyncId::ServerId(2.into());
    let active_folder_id = SyncId::ServerId(3.into());
    let objects: Vec<Box<dyn CloudObject>> = vec![
        Box::new(mock_trashed_cloud_folder(
            trashed_folder_id,
            "Trashed Folder".into(),
            None,
        )),
        Box::new(mock_cloud_folder(
            child_folder_id,
            "Child in Trashed Folder".into(),
            Some(trashed_folder_id),
        )),
        Box::new(mock_cloud_folder(
            active_folder_id,
            "Top-level Folder".into(),
            None,
        )),
    ];

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, objects);
        cloud_model.read(&app, |model, _| {
            let active = model.active_object_uids();
            assert_eq!(active, naive_active_object_uids(model));
            assert_eq!(active.len(), 1);
            assert!(active.contains(&active_folder_id.uid()));
        });
    });
}

#[test]
fn active_object_uids_matches_naive_with_nested_trashed_folder() {
    // folder_a (trashed) -> folder_b (not trashed) -> child folder (not trashed)
    // Both folder_b and child folder should be indirectly trashed.
    let folder_a_id = SyncId::ServerId(1.into());
    let folder_b_id = SyncId::ServerId(2.into());
    let child_folder_id = SyncId::ServerId(3.into());
    let active_folder_id = SyncId::ServerId(4.into());
    let objects: Vec<Box<dyn CloudObject>> = vec![
        Box::new(mock_trashed_cloud_folder(
            folder_a_id,
            "Folder A (trashed)".into(),
            None,
        )),
        Box::new(mock_cloud_folder(
            folder_b_id,
            "Folder B".into(),
            Some(folder_a_id),
        )),
        Box::new(mock_cloud_folder(
            child_folder_id,
            "Deeply nested".into(),
            Some(folder_b_id),
        )),
        Box::new(mock_cloud_folder(active_folder_id, "Active".into(), None)),
    ];

    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, objects);
        cloud_model.read(&app, |model, _| {
            let active = model.active_object_uids();
            assert_eq!(active, naive_active_object_uids(model));
            assert_eq!(active.len(), 1);
            assert!(active.contains(&active_folder_id.uid()));
        });
    });
}

#[test]
fn active_object_uids_matches_naive_with_empty_model() {
    App::test((), |mut app| async move {
        let cloud_model = create_cloud_model(&mut app, vec![]);
        cloud_model.read(&app, |model, _| {
            let active = model.active_object_uids();
            assert_eq!(active, naive_active_object_uids(model));
            assert!(active.is_empty());
        });
    });
}
