use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cloud_object_client::MockObjectClient;
use cloud_object_models::JsonSerializer;
use futures_lite::future;
use settings::{RespectUserSyncSetting, SyncToCloud};
use warp_core::features::FeatureFlag;
use warp_graphql::object_permissions::AccessLevel;
use warp_graphql::scalars::time::ServerTimestamp;
use warpui::{App, ModelHandle, SingletonEntity};

use super::{GetCloudObjectResponse, InitialLoadResponse, UpdateManager};
use crate::ASSETS;
use crate::auth::UserUid;
use crate::auth::user::TEST_USER_UID;
use crate::cloud_object::model::actions::{
    ObjectAction, ObjectActionHistory, ObjectActionSubtype, ObjectActionType, ObjectActions,
};
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::model::persistence::{CloudModel, CloudModelEvent, UpdateSource};
use crate::cloud_object::{
    BulkCreateCloudObjectResult, CloudModelType, CloudObjectEventEntrypoint, CloudObjectGuest,
    CloudObjectLocation, ConflictStatus, CreateCloudObjectResult, CreatedCloudObject,
    GenericCloudObject, GenericStringObjectFormat, JsonObjectType, ObjectDeleteResult,
    ObjectIdType, ObjectMetadataUpdateResult, ObjectPermissionsUpdateData, ObjectType, Owner,
    Revision, RevisionAndLastEditor, ServerCloudObject, ServerFolder, ServerGuestSubject,
    ServerNotebook, ServerObject, ServerObjectGuest, ServerPreference, Space,
    UpdateCloudObjectResult,
};
use crate::drive::CloudObjectTypeAndId;
use crate::drive::folders::{CloudFolder, CloudFolderModel, FolderId};
use crate::drive::sharing::{SharingAccessLevel, Subject, UserKind};
use crate::notebooks::{CloudNotebook, CloudNotebookModel, NotebookId};
use crate::persistence::ModelEvent;
use crate::server::cloud_objects::listener::ObjectUpdateMessage;
use crate::server::cloud_objects::test_utils::{
    UpdateManagerStruct, create_update_manager_struct, initialize_app, mock_server_api,
};
use crate::server::cloud_objects::update_manager::{
    FetchSingleObjectOption, GenericStringObjectInput, InitiatedBy, ServerMetadata,
    ServerPermissions, get_duplicate_object_name,
};
use crate::server::ids::{
    ClientId, HashableId, ObjectUid, ServerId, ServerIdAndType, SyncId, ToServerId,
};
use crate::server::sync_queue::SyncQueue;
use crate::settings::{CloudPreferenceModel, Preference};
use crate::workspaces::user_profiles::{UserProfileWithUID, UserProfiles};

fn create_object<K, M>(
    app: &mut App,
    update_manager: &ModelHandle<UpdateManager>,
    client_id: ClientId,
    model: M,
) -> GenericCloudObject<K, M>
where
    K: HashableId
        + ToServerId
        + std::fmt::Debug
        + Into<String>
        + Clone
        + Copy
        + Send
        + Sync
        + 'static,
    M: CloudModelType<IdType = K, CloudObjectType = GenericCloudObject<K, M>> + 'static,
{
    update_manager.update(app, |update_manager, ctx| {
        update_manager.create_object(
            model,
            Owner::mock_current_user(),
            client_id,
            CloudObjectEventEntrypoint::Unknown,
            true,
            None,
            InitiatedBy::User,
            ctx,
        );
    });
    CloudModel::handle(app).read(app, |cloud_model, _ctx| {
        cloud_model
            .get_object_of_type::<K, M>(&SyncId::ClientId(client_id))
            .expect("object should exist")
            .clone()
    })
}

fn create_object_result<K: HashableId + ToServerId>(
    client_id: ClientId,
    object_id: K,
    object_id_type: ObjectIdType,
) -> CreateCloudObjectResult {
    CreateCloudObjectResult::Success {
        created_cloud_object: CreatedCloudObject {
            client_id,
            revision_and_editor: RevisionAndLastEditor {
                revision: Revision::now(),
                last_editor_uid: Some("34jkaosdfj".to_string()),
            },
            metadata_ts: DateTime::<Utc>::default().into(),
            creator_uid: None,
            server_id_and_type: ServerIdAndType {
                id: object_id.to_server_id(),
                id_type: object_id_type,
            },
            permissions: ServerPermissions::mock_personal(),
        },
    }
}

fn create_notebook(
    app: &mut App,
    update_manager: &ModelHandle<UpdateManager>,
    client_id: ClientId,
) {
    update_manager.update(app, |update_manager, ctx| {
        update_manager.create_notebook(
            client_id,
            Owner::mock_current_user(),
            None,
            Default::default(),
            CloudObjectEventEntrypoint::Unknown,
            true,
            ctx,
        );
    });
}

fn receive_object_update_from_rtc(
    app: &mut App,
    update_manager: &ModelHandle<UpdateManager>,
    item: ObjectUpdateMessage,
) {
    update_manager.update(app, move |update_manager, ctx| {
        update_manager.received_message_from_server(item, ctx);
    })
}

fn receive_initial_load_or_polling_update(
    app: &mut App,
    update_manager: &ModelHandle<UpdateManager>,
    force_refresh: bool,
    mocked_response: InitialLoadResponse,
) {
    update_manager.update(app, move |update_manager, ctx| {
        update_manager.on_changed_objects_fetched(mocked_response, force_refresh, ctx);
    })
}

fn mock_server_permissions(owner: Owner) -> ServerPermissions {
    ServerPermissions {
        space: owner,
        guests: Vec::new(),
        anyone_link_sharing: None,
        permissions_last_updated_ts: Utc::now().into(),
    }
}

fn mock_server_notebook(id: NotebookId, owner: Owner, metadata: ServerMetadata) -> ServerNotebook {
    ServerNotebook::new(
        SyncId::ServerId(id.into()),
        CloudNotebookModel {
            title: format!("n{id}"),
            data: format!("n{id}"),
        },
        metadata,
        mock_server_permissions(owner),
    )
}

fn mock_server_folder(id: FolderId, owner: Owner, metadata: ServerMetadata) -> ServerFolder {
    ServerFolder::new(
        SyncId::ServerId(id.into()),
        CloudFolderModel {
            name: format!("f{id}"),
            is_open: false,
            is_warp_pack: false,
        },
        metadata,
        mock_server_permissions(owner),
    )
}

fn update_notebook(
    app: &mut App,
    update_manager: &ModelHandle<UpdateManager>,
    client_id: ClientId,
    title: String,
) {
    update_manager.update(app, |update_manager, ctx| {
        update_manager.update_notebook_title(Arc::new(title), SyncId::ClientId(client_id), ctx);
    });
}

#[track_caller]
fn assert_pending_online_only_change_for_object(app: &mut App, uid: &ObjectUid, status: bool) {
    CloudModel::handle(app).update(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_mut_by_uid(uid) {
            assert_eq!(
                object.metadata().has_pending_online_only_change(),
                status,
                "Expected has_pending_online_only_change for {uid} to be {status}"
            );
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_pending_status_for_object(app: &mut App, uid: &ObjectUid, status: bool) {
    CloudModel::handle(app).update(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_mut_by_uid(uid) {
            assert_eq!(
                object.metadata().has_pending_content_changes(),
                status,
                "Expected has_pending_content_changes for {uid} to be {status}"
            );
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_errored_status_for_object(app: &mut App, uid: &ObjectUid, is_errored: bool) {
    CloudModel::handle(app).update(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_mut_by_uid(uid) {
            assert_eq!(
                object.metadata().is_errored(),
                is_errored,
                "Expected error status for {uid} to be {is_errored}"
            );
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_trashed_status_for_object(app: &mut App, uid: &ObjectUid, is_trashed: bool) {
    CloudModel::handle(app).update(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_mut_by_uid(uid) {
            assert_eq!(
                object.metadata().trashed_ts.is_some(),
                is_trashed,
                "Expected trashed status for {uid} to be {is_trashed}"
            );
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_conflict_status_for_object(app: &mut App, uid: &ObjectUid, has_conflicts: bool) {
    CloudModel::handle(app).read(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_by_uid(uid) {
            assert_eq!(
                object.has_conflicting_changes(),
                has_conflicts,
                "Expected conflict status for {uid} to be {has_conflicts}"
            );
        } else {
            panic!("object {uid} should have been in cloud model, but wasn't");
        }
    })
}

fn assert_root_level_for_object(app: &mut App, uid: &ObjectUid, is_root_level: bool) {
    CloudModel::handle(app).update(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_mut_by_uid(uid) {
            assert_eq!(object.metadata().folder_id.is_none(), is_root_level);
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_folder_for_object(app: &App, uid: &ObjectUid, folder_id: Option<SyncId>) {
    CloudModel::handle(app).read(app, |cloud_model, _| {
        if let Some(object) = cloud_model.get_by_uid(uid) {
            assert_eq!(object.metadata().folder_id, folder_id);
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

fn assert_space_for_object(app: &App, uid: &ObjectUid, space: Space) {
    CloudModel::handle(app).read(app, |cloud_model, ctx| {
        if let Some(object) = cloud_model.get_by_uid(uid) {
            assert_eq!(object.space(ctx), space);
        } else {
            panic!("object should have been in cloud model, but wasn't");
        }
    });
}

/// Asserts that the current editor email address of the object matches the expected value.
fn assert_current_editor_for_object(app: &mut App, uid: &ObjectUid, current_editor: Option<&str>) {
    app.read(|ctx| {
        if let Some(object) = CloudModel::as_ref(ctx).get_by_uid(uid) {
            assert_eq!(
                object.metadata().current_editor_uid.as_deref(),
                current_editor
            );
        } else {
            panic!("object {uid} should have been in cloud model, but wasn't");
        }
    });
}

/// Asserts that the `metadata_last_updated_ts` timestamp for an object has the expected value.
fn assert_metadata_ts_for_object(app: &mut App, uid: &ObjectUid, metadata_ts: ServerTimestamp) {
    app.read(|ctx| {
        if let Some(object) = CloudModel::as_ref(ctx).get_by_uid(uid) {
            assert_eq!(
                object.metadata().metadata_last_updated_ts,
                Some(metadata_ts)
            );
        } else {
            panic!("object {uid} should have been in cloud model, but wasn't");
        }
    });
}

/// Asserts that the `revision` timestamp for an object has the expected value.
fn assert_revision_for_object(app: &App, uid: &ObjectUid, revision: impl Into<Revision>) {
    let revision = revision.into();
    app.read(|ctx| {
        if let Some(object) = CloudModel::as_ref(ctx).get_by_uid(uid) {
            assert_eq!(object.metadata().revision, Some(revision));
        } else {
            panic!("object {uid} should have been in cloud model, but wasn't");
        }
    });
}

fn assert_notebook_data(app: &App, sync_id: SyncId, expected_data: &str) {
    CloudModel::handle(app).read(app, |cloud_model, _| {
        match cloud_model.get_notebook(&sync_id) {
            Some(notebook) => assert_eq!(&notebook.model().data, expected_data),
            None => panic!("notebook {sync_id:?} should have been in cloud model, but wasn't"),
        }
    })
}

fn assert_notebook_conflict_data(app: &App, sync_id: SyncId, expected_data: &str) {
    CloudModel::handle(app).read(app, |cloud_model, _| {
        let notebook = cloud_model.get_notebook(&sync_id).unwrap_or_else(|| {
            panic!("notebook {sync_id:?} should have been in cloud model, but wasn't")
        });
        match &notebook.conflict_status {
            ConflictStatus::ConflictingChanges { object } => {
                assert_eq!(&object.model.data, expected_data)
            }
            ConflictStatus::NoConflicts => {
                panic!("notebook {sync_id:?} should have been in conflict")
            }
        }
    })
}

fn db_events(update_manager_struct: &UpdateManagerStruct) -> Vec<ModelEvent> {
    let mut db_events = Vec::new();

    while let Ok(event) = update_manager_struct.receiver.try_recv() {
        db_events.push(event);
    }

    db_events
}

fn cloud_events(update_manager_struct: &UpdateManagerStruct) -> Vec<CloudModelEvent> {
    let mut events = Vec::new();
    while let Ok(event) = update_manager_struct.cloud_model_events.try_recv() {
        events.push(event);
    }
    events
}

/// Add a folder to the cloud model.
fn add_folder(id: FolderId, owner: Owner, cloud_model: &mut CloudModel) {
    cloud_model.add_object(
        id.into(),
        CloudFolder::new_from_server(mock_server_folder(
            id,
            owner,
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
            },
        )),
    )
}

fn mock_create_notebook(
    client_id: ClientId,
    server_api: &mut MockObjectClient,
    notebook_id: NotebookId,
) {
    server_api
        .expect_create_notebook()
        .times(1)
        .return_once(move |_| {
            Ok(CreateCloudObjectResult::Success {
                created_cloud_object: CreatedCloudObject {
                    client_id,
                    revision_and_editor: RevisionAndLastEditor {
                        revision: Revision::now(),
                        last_editor_uid: Some("34jkaosdfj".to_string()),
                    },
                    metadata_ts: DateTime::<Utc>::default().into(),
                    server_id_and_type: ServerIdAndType {
                        id: notebook_id.to_server_id(),
                        id_type: ObjectIdType::Notebook,
                    },
                    creator_uid: None,
                    permissions: ServerPermissions::mock_personal(),
                },
            })
        });
}

fn mock_fetch_single_cloud_object(
    server_api: &mut MockObjectClient,
    notebook_id: NotebookId,
    server_id: ServerId,
) {
    server_api
        .expect_fetch_single_cloud_object()
        .times(1)
        .return_once(move |_| {
            Ok(GetCloudObjectResponse {
                object: ServerCloudObject::Notebook(ServerNotebook::new(
                    SyncId::ServerId(notebook_id.into()),
                    CloudNotebookModel {
                        title: "server notebook".to_string(),
                        data: "server notebook".to_string(),
                    },
                    ServerMetadata {
                        uid: server_id,
                        revision: Revision::now(),
                        metadata_last_updated_ts: Utc::now().into(),
                        trashed_ts: None,
                        folder_id: None,
                        is_welcome_object: false,
                        creator_uid: None,
                        last_editor_uid: None,
                        current_editor_uid: None,
                    },
                    ServerPermissions {
                        space: Owner::mock_current_user(),
                        guests: Vec::new(),
                        anyone_link_sharing: None,
                        permissions_last_updated_ts: Utc::now().into(),
                    },
                )),
                descendants: vec![],
                action_histories: vec![ObjectActionHistory {
                    uid: server_id.uid(),
                    hashed_sqlite_id: server_id.sqlite_type_and_uid_hash(ObjectIdType::Notebook),
                    latest_processed_at_timestamp: Utc::now(),
                    actions: vec![],
                }],
            })
        });
}

#[test]
fn test_sync_state_after_creation_item_not_in_sync_queue_folder() {
    App::test(ASSETS, |mut app| async move {
        let client_id = ClientId::new();
        initialize_app(&mut app);
        let object_id: FolderId = 123.into();
        let mut server_api = mock_server_api();
        server_api
            .expect_create_folder()
            .times(1)
            .return_once(move |_| {
                Ok(create_object_result(
                    client_id,
                    object_id,
                    ObjectIdType::Folder,
                ))
            });
        run_sync_state_after_creation_item_not_in_sync_queue(
            app,
            object_id.into(),
            CloudFolderModel::new("test folder", false),
            client_id,
            server_api,
        )
        .await;
    })
}

#[test]
fn test_sync_state_after_creation_item_not_in_sync_queue_notebook() {
    App::test(ASSETS, |mut app| async move {
        let client_id = ClientId::new();
        initialize_app(&mut app);

        let object_id: NotebookId = 123.into();
        let mut server_api = mock_server_api();
        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(create_object_result(
                    client_id,
                    object_id,
                    ObjectIdType::Notebook,
                ))
            });
        run_sync_state_after_creation_item_not_in_sync_queue(
            app,
            object_id.into(),
            CloudNotebookModel::default(),
            client_id,
            server_api,
        )
        .await;
    })
}

#[test]
fn test_sync_state_after_creation_item_not_in_sync_queue_generic_object() {
    App::test(ASSETS, |mut app| async move {
        let client_id = ClientId::new();
        initialize_app(&mut app);
        let object_id: GenericStringObjectId = 123.into();
        let mut server_api = mock_server_api();
        server_api
            .expect_create_generic_string_object()
            .times(1)
            .return_once(move |_, _, request| {
                assert!(request.serialized_model.is_some());
                Ok(create_object_result(
                    client_id,
                    object_id,
                    ObjectIdType::GenericStringObject,
                ))
            });
        run_sync_state_after_creation_item_not_in_sync_queue(
            app,
            object_id.into(),
            CloudPreferenceModel::new(
                Preference::new(
                    "foo".to_owned(),
                    "{\"test_key\": \"test_value\"}",
                    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                )
                .expect("error creating preference"),
            ),
            client_id,
            server_api,
        )
        .await;
    })
}

// Runs a test case where we validate the sync state of an object after it's been created.
async fn run_sync_state_after_creation_item_not_in_sync_queue<K, M>(
    mut app: App,
    object_id: ServerId,
    model: M,
    client_id: ClientId,
    server_api: MockObjectClient,
) where
    K: HashableId
        + ToServerId
        + std::fmt::Debug
        + Into<String>
        + Clone
        + Copy
        + Send
        + Sync
        + 'static,
    M: CloudModelType<IdType = K, CloudObjectType = GenericCloudObject<K, M>> + 'static,
{
    let server_id: SyncId = SyncId::ServerId(object_id);
    let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

    // create an object
    let object = create_object::<K, M>(
        &mut app,
        &update_manager_struct.update_manager,
        client_id,
        model,
    );

    // verify it's pending
    assert_pending_status_for_object(&mut app, &client_id.to_string(), true);

    // complete the create request
    SyncQueue::handle(&app)
        .update(&mut app, |sync_queue, ctx| {
            ctx.await_spawned_future(sync_queue.spawned_futures()[0])
        })
        .await;

    // because there aren't any items in the sync queue left for this object,
    // it should be marked as having no pending changes
    assert_pending_status_for_object(&mut app, &server_id.uid(), false);

    let events = db_events(&update_manager_struct);

    assert_eq!(events.len(), 4);
    // we created an object in the db
    assert_eq!(
        std::mem::discriminant(&events[0]),
        std::mem::discriminant(&M::upsert_event(
            object.upsert_params(object.model().object_type())
        ))
    );
    // when we got the correct response back from the server,
    // we updated the db with the server id
    assert!(matches!(
        &events[1],
        ModelEvent::UpdateObjectAfterServerCreation {
            client_id: _,
            server_creation_info: _
        }
    ));
    // since this object is no longer pending, we mark it as synced
    assert!(matches!(
        &events[2],
        ModelEvent::MarkObjectAsSynced {
            hashed_sqlite_id: _,
            revision_and_editor: _,
            metadata_ts: _,
        }
    ));
    assert!(matches!(
        &events[3],
        ModelEvent::SyncObjectActions { actions_to_sync: _ }
    ));
}

#[test]
fn test_sync_state_after_creation_item_in_flight() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();
        let server_id: SyncId = SyncId::ServerId(notebook_id.into());

        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: Revision::now(),
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                        server_id_and_type: ServerIdAndType {
                            id: notebook_id.to_server_id(),
                            id_type: ObjectIdType::Notebook,
                        },
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // verify it's pending
        assert_pending_status_for_object(&mut app, &client_id.to_string(), true);
        // update the notebook's title (but keep the server req in flight)
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "foo".to_string(),
        );
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                // stop dequeueing so we have time afterwards to assert the proper state
                sync_queue.stop_dequeueing();
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // there's still a sync queue item in flight for this object, so it should still be pending
        assert_pending_status_for_object(&mut app, &server_id.uid(), true);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 4);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered an update event in the db
        assert!(matches!(
            &events[1],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[2],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // we didn't send a MarkObjectAsSyncedEvent
    })
}

#[test]
fn test_sync_state_after_creation_fails_due_to_limit() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();
        let server_id = SyncId::ServerId(notebook_id.into());
        let team_uid: ServerId = ServerId::from(789);

        let mut create_notebook_calls = 0;
        server_api
            .expect_create_notebook()
            .times(2)
            .returning(move |_| {
                create_notebook_calls += 1;
                match create_notebook_calls {
                    // Return an over limit user error on the first attempt.
                    1 => Ok(CreateCloudObjectResult::UserFacingError(
                        "limit exceeded".to_string(),
                    )),
                    // Return a successful response on the second attempt.
                    2 => Ok(CreateCloudObjectResult::Success {
                        created_cloud_object: CreatedCloudObject {
                            client_id,
                            revision_and_editor: RevisionAndLastEditor {
                                revision: Revision::now(),
                                last_editor_uid: Some("34jkaosdfj".to_string()),
                            },
                            metadata_ts: DateTime::<Utc>::default().into(),
                            server_id_and_type: ServerIdAndType {
                                id: notebook_id.to_server_id(),
                                id_type: ObjectIdType::Notebook,
                            },
                            creator_uid: None,
                            permissions: ServerPermissions::mock_personal(),
                        },
                    }),
                    _ => unreachable!(),
                }
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook in team space
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.create_object(
                    CloudNotebookModel {
                        title: "notebook title".to_string(),
                        data: "echo hello world".to_string(),
                    },
                    Owner::Team { team_uid },
                    client_id,
                    CloudObjectEventEntrypoint::Unknown,
                    false,
                    None,
                    InitiatedBy::User,
                    ctx,
                );
            });

        // verify it's pending
        assert_pending_status_for_object(&mut app, &client_id.to_string(), true);

        // complete the notebook create request (this should fail due to limit hit)
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // complete the second notebook create request (in personal space)
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[1])
            })
            .await;

        let events = db_events(&update_manager_struct);

        // because there aren't any items in the sync queue left for this object,
        // it should be marked as having no pending changes
        assert_pending_status_for_object(&mut app, &server_id.uid(), false);

        assert_eq!(events.len(), 5);

        // We created a notebook in the db.
        match &events[0] {
            ModelEvent::UpsertNotebook { notebook } => {
                // Verify initial location of notebook is the shared drive.
                assert_eq!(notebook.permissions.owner, Owner::Team { team_uid });
            }
            _ => panic!("Expected an UpsertNotebook event"),
        }
        // We also triggered an update event in the db when moving it from team to personal drive.
        match &events[1] {
            ModelEvent::UpsertNotebook { notebook } => {
                // Verify new location of notebook is the personal drive.
                assert_eq!(notebook.permissions.owner, Owner::mock_current_user());
            }
            _ => panic!("Expected an UpsertNotebook event"),
        }
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[2],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        // since this object is no longer pending, we mark it as synced
        assert!(matches!(
            &events[3],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _,
            }
        ));
        assert!(matches!(
            &events[4],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
    })
}

#[test]
fn test_sync_state_after_update_item_not_in_sync_queue() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();
        let server_id: SyncId = SyncId::ServerId(notebook_id.into());

        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: Revision::now(),
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_id_and_type: ServerIdAndType {
                            id: notebook_id.to_server_id(),
                            id_type: ObjectIdType::Notebook,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::<ServerNotebook>::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: Revision::now(),
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // update the notebook's title
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "foo".to_string(),
        );

        // Make sure that we would warn about quitting at this point
        CloudModel::handle(&app).read(&app, |cloud_model, _| {
            assert_eq!(cloud_model.num_unsaved_objects(), 1);
            assert_eq!(
                cloud_model.num_unsaved_objects_to_warn_about_before_quitting(),
                1
            );
        });

        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // complete the notebook update request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[1])
            })
            .await;

        // because there aren't any items in the sync queue left for this object,
        // it should be marked as having no pending changes
        assert_pending_status_for_object(&mut app, &server_id.uid(), false);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 5);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered an update event in the db
        assert!(matches!(
            &events[1],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[2],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // since this object is no longer pending, we mark it as synced
        assert!(matches!(
            &events[4],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _,
            }
        ));

        // Because the update came from this instance, we report it as local.
        let events = cloud_events(&update_manager_struct);
        assert_eq!(
            events,
            vec![
                CloudModelEvent::ObjectCreated {
                    type_and_id: CloudObjectTypeAndId::Notebook(SyncId::ClientId(client_id)),
                },
                CloudModelEvent::ObjectForceExpanded {
                    id: client_id.to_string()
                },
                // The update was made using the client ID.
                CloudModelEvent::ObjectUpdated {
                    type_and_id: CloudObjectTypeAndId::Notebook(SyncId::ClientId(client_id)),
                    source: UpdateSource::Local
                },
                CloudModelEvent::ObjectSynced {
                    type_and_id: CloudObjectTypeAndId::Notebook(SyncId::ServerId(
                        notebook_id.to_server_id()
                    )),
                    client_id,
                    server_id: notebook_id.to_server_id(),
                },
            ]
        );
    })
}

#[test]
fn test_create_sets_editor() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = mock_server_api();
        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();

        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(create_object_result(
                    client_id,
                    notebook_id,
                    ObjectIdType::Notebook,
                ))
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Create the notebook.
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);

        // Even locally, it should have an editor.
        assert_current_editor_for_object(&mut app, &client_id.to_string(), Some(TEST_USER_UID));

        // Complete the notebook create request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Now, the notebook should have a server id and editor.
        assert_current_editor_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            Some(TEST_USER_UID),
        );
    });
}

#[test]
fn test_bulk_create_generic_string_objects() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let client_id_1 = ClientId::new();
        let object_id_1: GenericStringObjectId = 123.into();
        let client_id_2 = ClientId::new();
        let object_id_2: GenericStringObjectId = 456.into();

        server_api
            .expect_bulk_create_generic_string_objects()
            .times(1)
            .return_once(move |_, _| {
                Ok(BulkCreateCloudObjectResult::Success {
                    created_cloud_objects: vec![
                        CreatedCloudObject {
                            client_id: client_id_1,
                            revision_and_editor: RevisionAndLastEditor {
                                revision: Revision::now(),
                                last_editor_uid: Some("34jkaosdfj".to_string()),
                            },
                            metadata_ts: DateTime::<Utc>::default().into(),
                            server_id_and_type: ServerIdAndType {
                                id: object_id_1.to_server_id(),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            creator_uid: None,
                            permissions: ServerPermissions::mock_personal(),
                        },
                        CreatedCloudObject {
                            client_id: client_id_2,
                            revision_and_editor: RevisionAndLastEditor {
                                revision: Revision::now(),
                                last_editor_uid: Some("34jkaosdfk".to_string()),
                            },
                            server_id_and_type: ServerIdAndType {
                                id: object_id_2.to_server_id(),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            metadata_ts: DateTime::<Utc>::default().into(),
                            creator_uid: None,
                            permissions: ServerPermissions::mock_personal(),
                        },
                    ],
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let inputs = vec![
            GenericStringObjectInput::<Preference, JsonSerializer> {
                id: client_id_1,
                model: CloudPreferenceModel::new(
                    Preference::new(
                        "storage_key_1".to_string(),
                        "{\"test_key\": \"test_value_1\"}",
                        SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                    )
                    .expect("error creating preference"),
                ),
                initial_folder_id: None,
                entrypoint: CloudObjectEventEntrypoint::Unknown,
            },
            GenericStringObjectInput::<Preference, JsonSerializer> {
                id: client_id_2,
                model: CloudPreferenceModel::new(
                    Preference::new(
                        "storage_key_2".to_string(),
                        "{\"test_key\": \"test_value_2\"}",
                        SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                    )
                    .expect("error creating preference"),
                ),
                initial_folder_id: None,
                entrypoint: CloudObjectEventEntrypoint::Unknown,
            },
        ];

        // Bulk create objects
        update_manager_struct
            .update_manager
            .update(&mut app, move |update_manager, ctx| {
                update_manager.bulk_create_generic_string_objects(
                    Owner::mock_current_user(),
                    inputs,
                    ctx,
                );
            });

        // Make sure that we won't block quitting even though there are pending changes at this point.
        CloudModel::handle(&app).read(&app, |cloud_model, _| {
            assert_eq!(cloud_model.num_unsaved_objects(), 2);
            assert_eq!(
                cloud_model.num_unsaved_objects_to_warn_about_before_quitting(),
                0
            );
        });

        // complete the object create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 7);
        // we created two items in bulk in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertGenericStringObjects { .. }
        ));

        // when we got the correct responses back from the server,
        // we updated the db with the server id, for each object
        assert!(matches!(
            &events[1],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[2],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _,
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        assert!(matches!(
            &events[4],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[5],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _,
            }
        ));
        assert!(matches!(
            &events[6],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
    })
}

#[test]
fn test_sync_state_after_update_item_not_in_sync_queue_generic_string_object() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let client_id = ClientId::new();
        let object_id: GenericStringObjectId = 123.into();
        let server_id: SyncId = SyncId::ServerId(object_id.into());

        server_api
            .expect_create_generic_string_object()
            .times(1)
            .return_once(move |_, _, _| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: Revision::now(),
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_id_and_type: ServerIdAndType {
                            id: object_id.to_server_id(),
                            id_type: ObjectIdType::GenericStringObject,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });
        server_api
            .expect_update_generic_string_object()
            .times(1)
            .return_once(move |_, _, _| {
                Ok(UpdateCloudObjectResult::<Box<dyn ServerObject>>::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: Revision::now(),
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a test json object
        create_object(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            CloudPreferenceModel::new(
                Preference::new(
                    "foo".to_owned(),
                    "{\"test_key\": \"test_value\"}",
                    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                )
                .expect("error creating preference"),
            ),
        );
        // update the test json object's data
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_object(
                    CloudPreferenceModel::new(
                        Preference::new(
                            "foo".to_owned(),
                            "{\"test_key\": \"test_value_2\"}",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    ),
                    SyncId::ClientId(client_id),
                    None,
                    ctx,
                );
            });
        // complete the object create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // complete the object update request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[1])
            })
            .await;

        // because there aren't any items in the sync queue left for this object,
        // it should be marked as having no pending changes
        assert_pending_status_for_object(&mut app, &server_id.uid(), false);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 5);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertGenericStringObject { .. }
        ));
        // we also triggered an update event in the db
        assert!(matches!(
            &events[1],
            ModelEvent::UpsertGenericStringObject { .. }
        ));
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[2],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // since this object is no longer pending, we mark it as synced
        assert!(matches!(
            &events[4],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _,
            }
        ));
    })
}

#[test]
fn test_sync_state_after_update_item_in_sync_queue() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();
        let server_id: SyncId = SyncId::ServerId(notebook_id.into());

        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: Revision::now(),
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_id_and_type: ServerIdAndType {
                            id: notebook_id.to_server_id(),
                            id_type: ObjectIdType::Notebook,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::<ServerNotebook>::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: Revision::now(),
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // update the notebook's title
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "foo".to_string(),
        );
        // update the notebook's title again (but this will stay in flight)
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "different title".into(),
        );
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                // stop dequeueing so we don't immediately execute futures #2 and #3
                sync_queue.stop_dequeueing();
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // complete the notebook update request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                sync_queue.start_dequeueing(ctx); // force a dequeue of future #2
                sync_queue.stop_dequeueing(); // stop so we don't execute future #3

                ctx.await_spawned_future(sync_queue.spawned_futures()[1])
            })
            .await;

        // there's still a sync queue item in flight for this object, so it should still be pending
        assert_pending_status_for_object(&mut app, &server_id.uid(), true);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 5);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered an update event in the db
        assert!(matches!(
            &events[1],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered _another_ update event in the db
        assert!(matches!(
            &events[2],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[3],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        assert!(matches!(
            &events[4],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // we didn't send a MarkObjectAsSyncedEvent
    })
}

#[test]
fn test_sync_state_after_creation_failure_item_not_in_sync_queue() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let client_id = ClientId::new();

        server_api
            .expect_create_notebook()
            .times(4)
            .returning(move |_| Err(anyhow::anyhow!("create failed!")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // verify it's pending
        assert_pending_status_for_object(&mut app, &client_id.to_string(), true);
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // await long enough that all the sync queue retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        // because there aren't any items in the sync queue left for this object,
        // it should be marked as errored
        assert_errored_status_for_object(&mut app, &client_id.to_string(), true);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 2);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we incremented the retry count in the db
        assert!(matches!(&events[1], ModelEvent::IncrementRetryCount(_)));
    })
}

#[test]
fn test_sync_state_after_update_failure_item_in_sync_queue() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let client_id = ClientId::new();
        let notebook_id: NotebookId = 123.into();
        let server_id: SyncId = SyncId::ServerId(notebook_id.into());

        server_api
            .expect_create_notebook()
            .times(1)
            .return_once(move |_| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: Revision::now(),
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_id_and_type: ServerIdAndType {
                            id: notebook_id.to_server_id(),
                            id_type: ObjectIdType::Notebook,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });
        server_api
            .expect_update_notebook()
            // .times(8)
            .returning(move |_, _, _, _| Err(anyhow::anyhow!("update failed!")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // update the notebook's title
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "foo".to_string(),
        );
        // update the notebook's title again
        update_notebook(
            &mut app,
            &update_manager_struct.update_manager,
            client_id,
            "different title".into(),
        );
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                // stop dequeueing so we don't immediately execute futures #2 and #3
                sync_queue.stop_dequeueing();
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // complete the first notebook update request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                sync_queue.start_dequeueing(ctx); // force a dequeue of future #2
                sync_queue.stop_dequeueing(); // stop so we don't execute future #3

                ctx.await_spawned_future(sync_queue.spawned_futures()[1])
            })
            .await;

        // await long enough that all the sync queue retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        // because notebook updates are dependent on one another, all should have registered
        // as failed and there should be no updates left in the sync queue.
        // because there aren't any items in the sync queue left for this object,
        // it should be marked as errored (and errored is pending)
        assert_pending_status_for_object(&mut app, &server_id.uid(), true);
        assert_errored_status_for_object(&mut app, &server_id.uid(), true);

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 7);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered an update event in the db
        assert!(matches!(
            &events[1],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // we also triggered _another_ update event in the db
        assert!(matches!(
            &events[2],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // when we got the correct response back from the server,
        // we updated the db with the server id
        assert!(matches!(
            &events[3],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _,
            }
        ));
        assert!(matches!(
            &events[4],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // we incremented the retry count in the db after the first update
        assert!(matches!(&events[5], ModelEvent::IncrementRetryCount(_)));
        // we incremented the retry count in the db after the second update
        assert!(matches!(&events[6], ModelEvent::IncrementRetryCount(_)));
        // we didn't send a MarkObjectAsSyncedEvent
    })
}

#[test]
fn test_fetch_single_cloud_object_not_pending_no_overwrite() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let client_id = ClientId::new();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        mock_create_notebook(client_id, &mut server_api, notebook_id);
        mock_fetch_single_cloud_object(&mut server_api, notebook_id, server_id);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Flush cloud model events.
        let _ = cloud_events(&update_manager_struct);

        // call to fetch the server's representation of this object.
        // because our in-memory notebook doesn't have any pending changes,
        // this will simply overwrite it with the server version.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                let fetch_cloud_object_rx = update_manager.fetch_single_cloud_object(
                    &server_id,
                    FetchSingleObjectOption::None,
                    ctx,
                );
                std::mem::drop(fetch_cloud_object_rx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_notebook_data(&app, sync_id, "server notebook");

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 6);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // the successful create triggered a set of the server id
        assert!(matches!(
            &events[1],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        // because the object had no in flight requests, it was marked as synced in the db
        assert!(matches!(
            &events[2],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // lastly, we upserted the notebook when we got the server version back
        assert!(matches!(
            &events[4],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        assert!(matches!(
            &events[5],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));

        // We emitted an event that the notebook changed.
        let events = cloud_events(&update_manager_struct);
        assert_eq!(
            events,
            vec![CloudModelEvent::ObjectUpdated {
                type_and_id: CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook),
                source: UpdateSource::Server
            }]
        );
    })
}

#[test]
fn test_fetch_single_cloud_object_pending_no_overwrite() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let client_id = ClientId::new();
        let sync_id = SyncId::ServerId(server_id);

        mock_create_notebook(client_id, &mut server_api, notebook_id);
        mock_fetch_single_cloud_object(&mut server_api, notebook_id, server_id);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                // stop dequeueing so we have time afterwards to assert the proper state
                sync_queue.stop_dequeueing();

                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // update the notebook (but keep the request in flight)
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data(
                    "client notebook 2".to_string().into(),
                    sync_id,
                    ctx,
                );
            });

        // Flush cloud model events.
        let _ = cloud_events(&update_manager_struct);

        // call to fetch the server's representation of this object.
        // our in-memory notebook has a pending change, so this will simply set its conflict_status
        // with the server's object, as we specified not to force an overwrite.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                let fetch_cloud_object_rx = update_manager.fetch_single_cloud_object(
                    &server_id,
                    FetchSingleObjectOption::None,
                    ctx,
                );
                std::mem::drop(fetch_cloud_object_rx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_notebook_data(&app, sync_id, "client notebook 2");

        // Because the change wasn't overwritten, no event is emitted.
        assert!(cloud_events(&update_manager_struct).is_empty());

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 7);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // the successful create triggered a set of the server id
        assert!(matches!(
            &events[1],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        // because the object had no in flight requests, it was marked as synced in the db
        assert!(matches!(
            &events[2],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // updating the notebook caused another upsert in the db
        assert!(matches!(
            &events[4],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // lastly, we upserted the notebook when we got the server version back
        // (though it's basically a no-op)
        assert!(matches!(
            &events[5],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        assert!(matches!(
            &events[6],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
    })
}

#[test]
fn test_fetch_single_cloud_object_pending_with_overwrite() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let client_id = ClientId::new();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(server_id);

        mock_create_notebook(client_id, &mut server_api, notebook_id);
        mock_fetch_single_cloud_object(&mut server_api, notebook_id, server_id);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // create a notebook
        create_notebook(&mut app, &update_manager_struct.update_manager, client_id);
        // complete the notebook create request
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                // stop dequeueing so we have time afterwards to assert the proper state
                sync_queue.stop_dequeueing();

                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;
        // update the notebook (but keep the request in flight)
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data(
                    "client notebook 2".to_string().into(),
                    sync_id,
                    ctx,
                );
            });

        // Flush cloud events.
        let _ = cloud_events(&update_manager_struct);

        // call to fetch the server's representation of this object.
        // our in-memory notebook has a pending change, but since we specified to force overwrite,
        // those changes will be lost and we'll reset back to the server object.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                let fetch_cloud_object_rx = update_manager.fetch_single_cloud_object(
                    &server_id,
                    FetchSingleObjectOption::ForceOverwrite,
                    ctx,
                );
                std::mem::drop(fetch_cloud_object_rx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_notebook_data(&app, sync_id, "server notebook");

        // There should be an update event for the overwritten data.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUpdated {
                type_and_id: CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook),
                source: UpdateSource::Server
            }]
        );

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 7);
        // we created a notebook in the db
        assert!(matches!(
            &events[0],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // the successful create triggered a set of the server id
        assert!(matches!(
            &events[1],
            ModelEvent::UpdateObjectAfterServerCreation {
                client_id: _,
                server_creation_info: _
            }
        ));
        // because the object had no in flight requests, it was marked as synced in the db
        assert!(matches!(
            &events[2],
            ModelEvent::MarkObjectAsSynced {
                hashed_sqlite_id: _,
                revision_and_editor: _,
                metadata_ts: _
            }
        ));
        assert!(matches!(
            &events[3],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
        // updating the notebook caused another upsert in the db
        assert!(matches!(
            &events[4],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        // lastly, we upserted the notebook when we got the server version back
        // (though it's basically a no-op)
        assert!(matches!(
            &events[5],
            ModelEvent::UpsertNotebook { notebook: _ }
        ));
        assert!(matches!(
            &events[6],
            ModelEvent::SyncObjectActions { actions_to_sync: _ }
        ));
    })
}

#[test]
fn test_metadata_after_trash_item_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        server_api
            .expect_trash_object()
            .times(1)
            .return_once(move |_| Ok(true));

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        // trash the notebook
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.trash_object(type_and_id, ctx);
            });

        // Since we optimistically update trashed_ts, we should emit an event before the future
        // completes.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectTrashed {
                type_and_id,
                source: UpdateSource::Local
            }]
        );
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);

        // There shouldn't be a duplicate server event once trashing succeeds.
        assert!(cloud_events(&update_manager_struct).is_empty());
    });
}

#[test]
fn test_pending_metadata_update_with_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id: SyncId = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: Some("ian@warp.dev".to_string()),
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // trash the notebook, but don't await the request.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.trash_object(type_and_id, ctx);
            });

        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            true,
        );

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);

        // While this trash request is "in-flight", mock getting an RTC update from the server that includes a new editor
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let mocked_metadata_update_message = ObjectUpdateMessage::ObjectMetadataChanged {
            metadata: mocked_metadata.clone(),
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_metadata_update_message,
        );

        // Assert we don't have a pending change now that we've updated the metadata
        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        // Assert that the metadata changes are correctly applied
        CloudModel::handle(&app).read(&app, |cloud_model, _ctx| {
            if let Some(object) = cloud_model.get_by_uid(&notebook_id.to_server_id().uid()) {
                assert_eq!(
                    object.metadata().current_editor_uid,
                    mocked_metadata.current_editor_uid
                );
                assert_eq!(
                    object
                        .metadata()
                        .metadata_last_updated_ts
                        .expect("metadata should exist"),
                    mocked_metadata.metadata_last_updated_ts
                );
            } else {
                panic!("object should have been in cloud model, but wasn't");
            }
        });

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 1);
        // we trigger a metadata update event from the rtc message
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id: _, metadata: _ }
        ));
        // No upsert should happen now since there's still the trash in flight
    })
}

#[test]
fn test_metadata_update_with_rtc_no_pending() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id: SyncId = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);

        // While this trash request is "in-flight", mock getting an RTC update from the server that includes a new editor
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: Some("ian@warp.dev".to_string()),
        };

        let mocked_metadata_update_message = ObjectUpdateMessage::ObjectMetadataChanged {
            metadata: mocked_metadata.clone(),
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_metadata_update_message,
        );

        // Assert we still don't have a pending change now that we've updated the metadata
        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        // Assert that the metadata changes are correctly applied
        CloudModel::handle(&app).read(&app, |cloud_model, _ctx| {
            if let Some(object) = cloud_model.get_by_uid(&notebook_id.to_server_id().uid()) {
                assert_eq!(
                    object.metadata().current_editor_uid,
                    mocked_metadata.current_editor_uid
                );
                assert_eq!(
                    object
                        .metadata()
                        .metadata_last_updated_ts
                        .expect("metadata should exist"),
                    mocked_metadata.metadata_last_updated_ts
                );
            } else {
                panic!("object should have been in cloud model, but wasn't");
            }
        });

        let events = db_events(&update_manager_struct);

        assert_eq!(events.len(), 1);
        // we trigger a metadata update event from the rtc message
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id: _, metadata: _ }
        ));
    })
}

#[test]
fn test_metadata_after_trash_item_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), notebook_metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        server_api
            .expect_trash_object()
            .returning(move |_| Err(anyhow::anyhow!("trash failed!")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);

        // trash the notebook
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.trash_object(type_and_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // await long enough that all the trash object retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);

        // There should be one event for the optimistic update and one for undoing it.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![
                CloudModelEvent::ObjectTrashed {
                    type_and_id,
                    source: UpdateSource::Local
                },
                CloudModelEvent::ObjectUntrashed {
                    type_and_id,
                    source: UpdateSource::Local
                }
            ]
        )
    });
}

#[test]
fn test_pending_metadata_update_with_polling() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let notebood_server_id: ServerId = 123.into();
        let notebook_id: NotebookId = notebood_server_id.into();
        let sync_id: SyncId = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), metadata);

        server_api
            .expect_trash_object()
            .times(1)
            .return_once(move |_| Ok(true));

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // trash the notebook, but don't await the request.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.trash_object(type_and_id, ctx);
            });
        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            true,
        );

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        // While this trash request is "in-flight", mock getting a polling update from the server that includes a new editor
        let mocked_metadata = ServerMetadata {
            uid: notebood_server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: Some("ian@warp.dev".to_string()),
        };

        // Add a generic object just for kicks and make sure it gets upserted
        let generic_object_server_id = 456.into();
        let mut updated_generic_string_objects: HashMap<
            GenericStringObjectFormat,
            Vec<Box<dyn ServerObject>>,
        > = HashMap::new();
        updated_generic_string_objects.insert(
            GenericStringObjectFormat::Json(JsonObjectType::Preference),
            vec![Box::new(ServerPreference::new(
                SyncId::ServerId(generic_object_server_id),
                CloudPreferenceModel::new(
                    Preference::new(
                        "test_storage_key".to_string(),
                        "{\"test_key\": \"test_value\"}",
                        SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                    )
                    .expect("error creating preference"),
                ),
                mocked_metadata.clone(),
                mock_server_permissions(Owner::mock_current_user()),
            ))],
        );

        let mocked_response = InitialLoadResponse {
            updated_notebooks: vec![ServerNotebook::new(
                SyncId::ServerId(notebood_server_id),
                CloudNotebookModel {
                    title: "".into(),
                    data: "".into(),
                },
                mocked_metadata.clone(),
                mock_server_permissions(Owner::mock_current_user()),
            )],
            deleted_notebooks: vec![],
            updated_folders: vec![],
            deleted_folders: vec![],
            user_profiles: vec![],
            updated_generic_string_objects,
            deleted_generic_string_objects: Default::default(),
            action_histories: Default::default(),
        };
        receive_initial_load_or_polling_update(
            &mut app,
            &update_manager_struct.update_manager,
            false, /* force_refresh */
            mocked_response,
        );

        // Assert we don't have a pending change now that we've updated the metadata
        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        // Assert that the metadata changes are correctly applied
        CloudModel::handle(&app).read(&app, |cloud_model, _ctx| {
            if let Some(object) = cloud_model.get_by_uid(&notebook_id.to_server_id().uid()) {
                assert_eq!(
                    object.metadata().current_editor_uid,
                    mocked_metadata.current_editor_uid
                );
                assert_eq!(
                    object
                        .metadata()
                        .metadata_last_updated_ts
                        .expect("metadata should exist"),
                    mocked_metadata.metadata_last_updated_ts
                );
            } else {
                panic!("object should have been in cloud model, but wasn't");
            }
        });

        let events = db_events(&update_manager_struct);

        // All the upserts from polling
        assert!(matches!(&events[0], ModelEvent::SyncObjectActions { .. }));
        assert!(matches!(&events[1], ModelEvent::UpsertNotebooks(_)));
        assert!(matches!(&events[2], ModelEvent::UpsertFolders(_)));
        assert!(matches!(&events[3], ModelEvent::DeleteObjects { ids: _ }));
    });
}

#[test]
fn test_metadata_update_with_polling_no_pending() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id: SyncId = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        // While this trash request is "in-flight", mock getting a polling update from the server that includes a new editor
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: Some("ian@warp.dev".to_string()),
        };
        let mocked_response = InitialLoadResponse {
            updated_notebooks: vec![ServerNotebook::new(
                SyncId::ServerId(server_id),
                CloudNotebookModel {
                    title: "".into(),
                    data: "".into(),
                },
                mocked_metadata.clone(),
                mock_server_permissions(Owner::mock_current_user()),
            )],
            deleted_notebooks: vec![],
            updated_folders: vec![],
            deleted_folders: vec![],
            user_profiles: vec![],
            updated_generic_string_objects: Default::default(),
            deleted_generic_string_objects: Default::default(),
            action_histories: Default::default(),
        };
        receive_initial_load_or_polling_update(
            &mut app,
            &update_manager_struct.update_manager,
            false, /* force_refresh */
            mocked_response,
        );

        // Assert we still don't have a pending change now that we've updated the metadata
        assert_pending_online_only_change_for_object(
            &mut app,
            &notebook_id.to_server_id().uid(),
            false,
        );

        // Assert that the metadata changes are correctly applied
        CloudModel::handle(&app).read(&app, |cloud_model, _ctx| {
            if let Some(object) = cloud_model.get_by_uid(&notebook_id.to_server_id().uid()) {
                assert_eq!(
                    object.metadata().current_editor_uid,
                    mocked_metadata.current_editor_uid
                );
                assert_eq!(
                    object
                        .metadata()
                        .metadata_last_updated_ts
                        .expect("metadata should exist"),
                    mocked_metadata.metadata_last_updated_ts
                );
            } else {
                panic!("object should have been in cloud model, but wasn't");
            }
        });

        let events = db_events(&update_manager_struct);

        // All the upserts from polling
        assert!(matches!(&events[0], ModelEvent::SyncObjectActions { .. }));
        assert!(matches!(&events[1], ModelEvent::UpsertNotebooks(_)));
        assert!(matches!(&events[2], ModelEvent::UpsertFolders(_)));
        assert!(matches!(&events[3], ModelEvent::DeleteObjects { ids: _ }));
    });
}

#[test]
fn test_metadata_after_untrash_item_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: Some(ServerTimestamp::from_unix_timestamp_micros(10).unwrap()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let mut untrashed_metadata = notebook_metadata;
        untrashed_metadata.trashed_ts = None;

        server_api
            .expect_untrash_object()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectMetadataUpdateResult::Success {
                    metadata: Box::new(untrashed_metadata),
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);

        // untrash the notebook
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.untrash_object(type_and_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);

        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUntrashed {
                type_and_id,
                source: UpdateSource::Local
            }]
        );
    })
}

#[test]
fn test_metadata_after_untrash_item_and_move_to_root() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: Some(ServerTimestamp::from_unix_timestamp_micros(10).unwrap()),
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let mut untrashed_metadata = notebook_metadata.clone();
        untrashed_metadata.trashed_ts = None;
        untrashed_metadata.folder_id = None;

        server_api
            .expect_untrash_object()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectMetadataUpdateResult::Success {
                    metadata: Box::new(untrashed_metadata),
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);
        assert_root_level_for_object(&mut app, &sync_id.uid(), false);

        // untrash the notebook
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.untrash_object(type_and_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);

        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUntrashed {
                type_and_id,
                source: UpdateSource::Local
            }]
        );
    })
}

#[test]
fn test_metadata_after_untrash_item_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: Some(ServerTimestamp::from_unix_timestamp_micros(10).unwrap()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), notebook_metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        // mock an unsuccessful untrashing attempt
        server_api
            .expect_untrash_object()
            .times(1)
            .return_once(move |_| Ok(ObjectMetadataUpdateResult::Failure));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // unsuccessfully attempt to untrash the notebook
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.untrash_object(type_and_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // check that object is still in trash
        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);

        // We do not optimistically update trashed_ts when untrashing, so there should be no event.
        assert!(cloud_events(&update_manager_struct).is_empty());
    })
}

#[test]
fn test_metadata_after_optimistic_grab_baton_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let notebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        // Mock a successful baton grab.
        let mut grab_metadata = notebook_metadata.clone();
        grab_metadata.current_editor_uid = Some(TEST_USER_UID.to_string());
        grab_metadata.metadata_last_updated_ts = (initial_ts + chrono::Duration::seconds(1)).into();
        server_api
            .expect_grab_notebook_edit_access()
            .times(1)
            .return_once(move |_| Ok(grab_metadata));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Grab the baton optimistically.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.grab_notebook_edit_access(sync_id, true, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_current_editor_for_object(&mut app, &sync_id.uid(), Some(TEST_USER_UID));
        // Ensure that the metadata timestamp from the server response is used.
        assert_metadata_ts_for_object(
            &mut app,
            &sync_id.uid(),
            (initial_ts + chrono::Duration::seconds(1)).into(),
        );

        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ModelEvent::UpdateObjectMetadata { id, metadata } => {
                assert_eq!(id, &sync_id.sqlite_uid_hash(ObjectIdType::Notebook));
                assert_eq!(metadata.current_editor_uid.as_deref(), Some(TEST_USER_UID));
            }
            _ => panic!("Expected an UpdateObjectMetadata event"),
        }
    });
}

#[test]
fn test_metadata_after_optimistic_grab_baton_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let notebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        // Mock a failed baton grab.
        server_api
            .expect_grab_notebook_edit_access()
            .times(1)
            .return_once(move |_| Err(anyhow::anyhow!("Baton grab failed")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Grab the baton optimistically.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.grab_notebook_edit_access(sync_id, true, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // On failure, the editor is still updated, but the metadata timestamp is not - future
        // metadata updates would override it.
        assert_current_editor_for_object(&mut app, &sync_id.uid(), Some(TEST_USER_UID));
        assert_metadata_ts_for_object(&mut app, &sync_id.uid(), initial_ts.into());

        let events = db_events(&update_manager_struct);
        assert!(events.is_empty());
    });
}

#[test]
fn test_metadata_after_non_optimistic_grab_baton_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let notebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        // Mock a successful baton grab.
        let mut grab_metadata = notebook_metadata.clone();
        grab_metadata.current_editor_uid = Some(TEST_USER_UID.to_string());
        grab_metadata.metadata_last_updated_ts = (initial_ts + chrono::Duration::seconds(1)).into();
        server_api
            .expect_grab_notebook_edit_access()
            .times(1)
            .return_once(move |_| Ok(grab_metadata));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Grab the baton optimistically.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.grab_notebook_edit_access(sync_id, false, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_current_editor_for_object(&mut app, &sync_id.uid(), Some(TEST_USER_UID));
        // Ensure that the metadata timestamp from the server response is used.
        assert_metadata_ts_for_object(
            &mut app,
            &sync_id.uid(),
            (initial_ts + chrono::Duration::seconds(1)).into(),
        );

        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ModelEvent::UpdateObjectMetadata { id, metadata } => {
                assert_eq!(id, &sync_id.sqlite_uid_hash(ObjectIdType::Notebook));
                assert_eq!(metadata.current_editor_uid.as_deref(), Some(TEST_USER_UID));
            }
            _ => panic!("Expected an UpdateObjectMetadata event"),
        }
    });
}

#[test]
fn test_metadata_after_non_optimistic_grab_baton_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let notebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        // Mock a failed baton grab.
        server_api
            .expect_grab_notebook_edit_access()
            .times(1)
            .return_once(move |_| Err(anyhow::anyhow!("Baton grab failed")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Grab the baton non-optimistically.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.grab_notebook_edit_access(sync_id, false, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // On failure, neither the editor nor the metadata timestamp are updated.
        assert_current_editor_for_object(&mut app, &sync_id.uid(), None);
        assert_metadata_ts_for_object(&mut app, &sync_id.uid(), initial_ts.into());

        let events = db_events(&update_manager_struct);
        assert!(events.is_empty());
    });
}

#[test]
fn test_report_initial_load() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Before the initial load, the listener should be pending.
        let mut listener = Box::pin(
            update_manager_struct
                .update_manager
                .read(&app, |update_manager, _| {
                    update_manager.initial_load_complete()
                }),
        );
        assert!(future::poll_once(&mut listener).await.is_none());

        receive_initial_load_or_polling_update(
            &mut app,
            &update_manager_struct.update_manager,
            false, /* force_refresh */
            InitialLoadResponse {
                updated_notebooks: Default::default(),
                deleted_notebooks: Default::default(),
                updated_folders: Default::default(),
                deleted_folders: Default::default(),
                user_profiles: Default::default(),
                updated_generic_string_objects: Default::default(),
                deleted_generic_string_objects: Default::default(),
                action_histories: Default::default(),
            },
        );

        // Afterwards, the listener should get notified.
        assert!(future::poll_once(listener).await.is_some());

        // Subsequent listeners should complete immediately.
        let listener = update_manager_struct
            .update_manager
            .read(&app, |update_manager, _| {
                update_manager.initial_load_complete()
            });
        assert!(future::poll_once(listener).await.is_some());
    });
}

#[test]
fn test_get_duplicate_object_name() {
    assert_eq!(
        get_duplicate_object_name("my object name"),
        "my object name (1)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (1)"),
        "my object name (2)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (23)"),
        "my object name (24)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name(1234)"),
        "my object name(1234) (1)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (0)"),
        "my object name (1)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (-3)"),
        "my object name (-3) (1)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (18446744073709551615)"),
        "my object name (18446744073709551615) (1)"
    );
    assert_eq!(
        get_duplicate_object_name("my object name (18446744073709551616)"),
        "my object name (18446744073709551616) (1)"
    );
}

#[test]
fn test_replace_object_with_conflicts() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let initial_metadata_ts = Utc::now();

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: initial_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let mut server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        // Mock a rejected update.
        server_notebook.metadata.revision =
            (initial_metadata_ts + chrono::Duration::seconds(1)).into();
        server_notebook.model.data = "conflicting update".into();
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::Rejected {
                    object: server_notebook.clone(),
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data("local edit".to_string().into(), sync_id, ctx);
            });
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);

        // The local edits should have been saved to SQLite, even if they conflict.
        let model_events = db_events(&update_manager_struct);
        assert!(matches!(
            &model_events[0],
            ModelEvent::UpsertNotebook { .. }
        ));

        // The local edit should emit a model update.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUpdated {
                type_and_id,
                source: UpdateSource::Local
            }]
        );

        // Complete the notebook update request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Once the update is rejected, the object should have a conflict status.
        // Note that the pending object status of the object is set to false since any further syncing of the object
        // will be rejected.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "local edit");

        // There shouldn't be any additional database updates in the meantime.
        assert!(db_events(&update_manager_struct).is_empty());
        assert!(cloud_events(&update_manager_struct).is_empty());

        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.replace_object_with_conflict(&sync_id.uid(), ctx);
            });

        // Accepting the server version should reset the pending and conflict statuses.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), false);
        assert_notebook_data(&app, sync_id, "conflicting update");

        // The server version should be saved to SQLite.
        let model_events = db_events(&update_manager_struct);
        assert!(matches!(
            &model_events[0],
            ModelEvent::UpsertNotebook { .. }
        ));

        // The server version should be reported as a new update.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUpdated {
                type_and_id,
                source: UpdateSource::Server
            }]
        );
    });
}

#[test]
fn test_pending_conflict_correctly_clears_after_edits() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let initial_metadata_ts = Utc::now();

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: initial_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let mut server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        //Mock a successful update.
        let new_revision = (initial_metadata_ts + chrono::Duration::seconds(2)).into();
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: new_revision,
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data("local edit".to_string().into(), sync_id, ctx);
            });
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);

        // The local edits should have been saved to SQLite
        let model_events = db_events(&update_manager_struct);
        assert!(matches!(
            &model_events[0],
            ModelEvent::UpsertNotebook { .. }
        ));

        // While we still have edits in flight, receive an RTC message, that has a TS before the object update
        // response. This should put the notebook conflict status as pending
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: (initial_metadata_ts + chrono::Duration::seconds(1)).into(),
            metadata_last_updated_ts: (initial_metadata_ts + chrono::Duration::seconds(1)).into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        // Mock a rejected update.
        server_notebook.metadata = mocked_metadata;
        server_notebook.model.data = "conflicting update".into();
        let mocked_update_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook.clone())),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_update_message,
        );

        // Assert we have a conflict status
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);

        // Complete the notebook update request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Once the update is rejected, the object should not have a conflict status.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), false);
        assert_notebook_data(&app, sync_id, "local edit");

        // Mock another incoming message from RTC, and validate that it goes through and we aren't in
        // conflict mode still
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: (initial_metadata_ts + chrono::Duration::seconds(4)).into(),
            metadata_last_updated_ts: (initial_metadata_ts + chrono::Duration::seconds(4)).into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        // Mock a rejected update.
        server_notebook.metadata = mocked_metadata;
        server_notebook.model.data = "a cool update".into();
        let mocked_update_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook)),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_update_message,
        );
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), false);
        assert_notebook_data(&app, sync_id, "a cool update");
    });
}

#[test]
fn test_pending_conflict_correctly_stays_after_edits() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let initial_metadata_ts = Utc::now();

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: initial_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let mut server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        //Mock a successful update.
        let new_revision = (initial_metadata_ts + chrono::Duration::seconds(2)).into();
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: new_revision,
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data("local edit".to_string().into(), sync_id, ctx);
            });
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);

        // The local edits should have been saved to SQLite
        let model_events = db_events(&update_manager_struct);
        assert!(matches!(
            &model_events[0],
            ModelEvent::UpsertNotebook { .. }
        ));

        // While we still have edits in flight, receive an RTC message, that has a TS after the object update
        // response. This should put the notebook conflict status as pending
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: (initial_metadata_ts + chrono::Duration::seconds(3)).into(),
            metadata_last_updated_ts: (initial_metadata_ts + chrono::Duration::seconds(3)).into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        // Mock a rejected update.
        server_notebook.metadata = mocked_metadata;
        server_notebook.model.data = "conflicting update".into();
        let mocked_update_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook.clone())),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_update_message,
        );

        // Complete the notebook update request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Once the update is rejected, the object should have a conflict status.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "local edit");

        // Mock another incoming message from RTC, and validate that we are still in conflict mode
        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: (initial_metadata_ts + chrono::Duration::seconds(4)).into(),
            metadata_last_updated_ts: (initial_metadata_ts + chrono::Duration::seconds(4)).into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        // Mock a rejected update.
        server_notebook.metadata = mocked_metadata;
        server_notebook.model.data = "conflicting update2 ".into();
        let mocked_update_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook)),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            mocked_update_message,
        );
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "local edit");
    });
}

#[test]
fn test_pending_self_conflict_clears_out_of_order() {
    // This tests the case where the client makes an edit and receives the RTC update for it
    // _before_ the server response to their edit.

    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = MockObjectClient::new();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();
        let edit_ts = initial_ts + chrono::Duration::seconds(2);

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: initial_ts.into(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let mut server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        // Mock the server response to an update.
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: edit_ts.into(),
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Make an edit.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data("edit".to_string().into(), sync_id, ctx);
            });
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);

        // Receive an RTC message for the update, with its same timestamp.
        server_notebook.metadata.revision = edit_ts.into();
        server_notebook.metadata.metadata_last_updated_ts = edit_ts.into();
        server_notebook.model.data = "edit".into();
        let edit_rtc_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook.clone())),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            edit_rtc_message,
        );

        // There should be both a conflict (from the RTC message) and a pending content change.
        // We will not show the conflict yet, since it _could_ be an echoed edit.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "edit");
        assert_notebook_conflict_data(&app, sync_id, "edit");

        // Complete the notebook update request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Once the update is complete, it clears the  conflict status.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), false);
        assert_notebook_data(&app, sync_id, "edit");
        assert_revision_for_object(&app, &sync_id.uid(), edit_ts);
    });
}

#[test]
fn test_pending_newer_conflict_remains_out_of_order() {
    // This tests the case where the client makes an edit which is eventually accepted by the
    // server, but receives an RTC update about a _newer_ edit in the meantime.
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = MockObjectClient::new();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let initial_ts = Utc::now();
        let local_edit_ts = initial_ts + chrono::Duration::seconds(2);
        let remote_edit_ts = initial_ts + chrono::Duration::seconds(3);

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: initial_ts.into(),
            metadata_last_updated_ts: initial_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        let mut server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        // Mock the server response to an update, which is accepted but stale.
        server_api
            .expect_update_notebook()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(UpdateCloudObjectResult::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: local_edit_ts.into(),
                        last_editor_uid: None,
                    },
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Make an edit.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.update_notebook_data("local edit".to_string().into(), sync_id, ctx);
            });
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);

        // Receive an RTC message for the newer edit.
        server_notebook.metadata.revision = remote_edit_ts.into();
        server_notebook.metadata.metadata_last_updated_ts = remote_edit_ts.into();
        server_notebook.model.data = "remote edit".into();
        let edit_rtc_message = ObjectUpdateMessage::ObjectContentChanged {
            server_object: Box::new(ServerCloudObject::Notebook(server_notebook.clone())),
            last_editor: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            edit_rtc_message,
        );

        // There should be both a conflict (from the RTC message) and a pending content change.
        // We would not show the conflict yet, since it _could_ be an echoed edit.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), true);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "local edit");
        assert_notebook_conflict_data(&app, sync_id, "remote edit");

        // Complete the notebook update request.
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Once the update is complete, the notebook remains in conflict with the newer edit.
        assert_pending_status_for_object(&mut app, &sync_id.uid(), false);
        assert_conflict_status_for_object(&mut app, &sync_id.uid(), true);
        assert_notebook_data(&app, sync_id, "local edit");
        assert_notebook_conflict_data(&app, sync_id, "remote edit");
        assert_revision_for_object(&app, &sync_id.uid(), local_edit_ts);
    });
}

#[test]
fn test_accepts_new_metadata_with_force_refresh() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id: SyncId = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let mocked_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: Some("BoogaBooga".to_string()),
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let mocked_response = InitialLoadResponse {
            updated_notebooks: vec![ServerNotebook::new(
                SyncId::ServerId(server_id),
                CloudNotebookModel {
                    title: "".into(),
                    data: "".into(),
                },
                mocked_metadata.clone(),
                mock_server_permissions(Owner::mock_current_user()),
            )],
            deleted_notebooks: vec![],
            updated_folders: vec![],
            deleted_folders: vec![],
            user_profiles: vec![],
            updated_generic_string_objects: Default::default(),
            deleted_generic_string_objects: Default::default(),
            action_histories: Default::default(),
        };

        // Force a sync for all objects
        receive_initial_load_or_polling_update(
            &mut app,
            &update_manager_struct.update_manager,
            true, /* force_refresh */
            mocked_response,
        );

        // Assert that the metadata changes are correctly applied
        CloudModel::handle(&app).read(&app, |cloud_model, _ctx| {
            if let Some(object) = cloud_model.get_by_uid(&notebook_id.to_server_id().uid()) {
                assert_eq!(object.metadata().creator_uid, mocked_metadata.creator_uid);
            } else {
                panic!("object should have been in cloud model, but wasn't");
            }
        });

        let events = db_events(&update_manager_struct);

        // All the upserts from polling
        assert!(matches!(&events[0], ModelEvent::SyncObjectActions { .. }));
        assert!(matches!(&events[1], ModelEvent::UpsertNotebooks(_)));
        assert!(matches!(&events[2], ModelEvent::UpsertFolders(_)));
        assert!(matches!(&events[3], ModelEvent::DeleteObjects { ids: _ }));
    });
}

#[test]
fn test_delete_single_object() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        // Create notebook object
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let ts = Utc::now();

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: ts.into(),
            trashed_ts: Some(ts.into()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        // Mock delete
        server_api
            .expect_delete_object()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectDeleteResult::Success {
                    deleted_ids: vec![SyncId::ServerId(notebook_id.into())],
                })
            });

        // Delete notebook
        let type_and_id = CloudObjectTypeAndId::Notebook(SyncId::ServerId(notebook_id.into()));
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.delete_object_by_user(type_and_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // Check that notebook is not in CloudModel anymore
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            assert!(
                !cloud_model.check_if_object_is_in_cloudmodel(notebook_id.to_server_id().uid()),
                "Deleted object should not be in CloudModel anymore"
            );
        });

        // Check that we reported the deletion.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectDeleted {
                type_and_id,
                folder_id: None
            }]
        );
    });
}

#[test]
fn test_empty_trash() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();
        let ts = Utc::now();

        // Create a few objects in the Personal space
        let notebook_server_id: ServerId = 123.into();
        let notebook_id: NotebookId = notebook_server_id.into();
        let notebook_sync_id = SyncId::ServerId(notebook_id.into());
        let notebook_metadata = ServerMetadata {
            uid: notebook_server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: ts.into(),
            trashed_ts: Some(ts.into()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), notebook_metadata);

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                notebook_sync_id,
                CloudNotebook::new_from_server(notebook.clone()),
            );
        });

        // Mock delete
        server_api
            .expect_empty_trash()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectDeleteResult::Success {
                    deleted_ids: vec![SyncId::ServerId(notebook_id.into())],
                })
            });

        // Empty trash
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.empty_trash(Space::Personal, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // Check that notebook is not in CloudModel anymore
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            assert!(
                !cloud_model.check_if_object_is_in_cloudmodel(notebook_id.to_server_id().uid()),
                "Deleted notebook should not be in CloudModel anymore"
            );
        });

        // Check that we reported the deletions.

        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectDeleted {
                type_and_id: CloudObjectTypeAndId::from_id_and_type(
                    notebook_sync_id,
                    ObjectType::Notebook
                ),
                folder_id: None
            },]
        );
    });
}

#[test]
fn test_leave_shared_object() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let sharing_team = Owner::Team {
            team_uid: 99.into(),
        };

        // Create a folder.
        let folder_server_id: ServerId = 456.into();
        let folder_id: FolderId = folder_server_id.into();
        let folder_sync_id = SyncId::ServerId(folder_server_id);
        let server_folder = mock_server_folder(
            folder_id,
            sharing_team,
            ServerMetadata {
                uid: folder_server_id,
                revision: Revision::now(),
                metadata_last_updated_ts: Utc::now().into(),
                trashed_ts: None,
                folder_id: None,
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                current_editor_uid: None,
            },
        );

        // Create a notebook in the folder.
        let notebook_server_id: ServerId = 123.into();
        let notebook_id: NotebookId = notebook_server_id.into();
        let notebook_sync_id = SyncId::ServerId(notebook_server_id);

        let server_notebook = mock_server_notebook(
            notebook_id,
            sharing_team,
            ServerMetadata {
                uid: notebook_server_id,
                revision: Revision::now(),
                metadata_last_updated_ts: Utc::now().into(),
                trashed_ts: None,
                folder_id: Some(folder_id),
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                current_editor_uid: None,
            },
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(folder_sync_id, CloudFolder::new_from_server(server_folder));
            cloud_model.add_object(
                notebook_sync_id,
                CloudNotebook::new_from_server(server_notebook),
            );
        });

        // Mock leaving the folder.
        server_api
            .expect_leave_object()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectDeleteResult::Success {
                    deleted_ids: vec![folder_sync_id],
                })
            });

        // Leave the folder.
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.leave_object(folder_server_id, ctx);
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // Check that neither object is in CloudModel.
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            assert!(
                !cloud_model.check_if_object_is_in_cloudmodel(folder_server_id.uid()),
                "Left object should not be in CloudModel anymore"
            );
            assert!(
                !cloud_model.check_if_object_is_in_cloudmodel(notebook_server_id.uid()),
                "Left object contents should not be in CloudModel anymore"
            );
        });

        // Check that we reported the deleted objects.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![
                CloudModelEvent::ObjectDeleted {
                    type_and_id: CloudObjectTypeAndId::Folder(folder_sync_id),
                    folder_id: None
                },
                CloudModelEvent::ObjectDeleted {
                    type_and_id: CloudObjectTypeAndId::Notebook(notebook_sync_id),
                    folder_id: Some(folder_sync_id)
                }
            ]
        );
    });
}

#[test]
fn test_record_object_action() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let timestamp = Utc::now();

        let hashed_object_id = "Notebook-asdfasdfasdfasdfasdf21".to_string();
        let hashed_object_id_clone = hashed_object_id.clone();

        let actions: Vec<ObjectAction> = vec![
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];
        server_api
            .expect_record_object_action()
            .times(1)
            .return_once(move |_, _, _, _| {
                Ok(ObjectActionHistory {
                    uid: hashed_object_id.clone(),
                    hashed_sqlite_id: hashed_object_id.clone(),
                    latest_processed_at_timestamp: timestamp,
                    actions,
                })
            });

        // Record object action
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.record_object_action(
                    CloudObjectTypeAndId::Notebook(SyncId::ServerId(ServerId::from_string_lossy(
                        "asdfasdfasdfasdfasdf21",
                    ))),
                    ObjectActionType::Execute,
                    None,
                    ctx,
                );
            });

        // Wait for the futures to finish
        SyncQueue::handle(&app)
            .update(&mut app, |sync_queue, ctx| {
                ctx.await_spawned_future(sync_queue.spawned_futures()[0])
            })
            .await;

        // Check that there are three actions stored.
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 3);
        });
    });
}

#[test]
fn test_overwrite_object_action_history_no_actions_on_client() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let timestamp = Utc::now();

        let hashed_object_id = "Notebook-asdf".to_string();
        let hashed_object_id_clone = hashed_object_id.clone();

        let actions: Vec<ObjectAction> = vec![
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];

        let mock_history = ObjectActionHistory {
            uid: hashed_object_id.clone(),
            hashed_sqlite_id: hashed_object_id.clone(),
            latest_processed_at_timestamp: timestamp,
            actions,
        };

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.maybe_overwrite_object_action_history(&mock_history, ctx);
            });

        // Check that these actions are accepted
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 3);
        });
    });
}

#[test]
fn test_overwrite_object_action_history_reject() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let timestamp = Utc::now();

        let hashed_object_id = "Notebook-asdf".to_string();
        let hashed_object_id_clone = hashed_object_id.clone();

        // the server's most recent action was 1 minute ago
        let server_actions: Vec<ObjectAction> = vec![
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(1),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(1)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(12),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(12)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];

        // The client actions have one action that is more recent than what the server is sending.
        let client_actions: Vec<ObjectAction> = vec![
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(1),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(1)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(12),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(12)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];

        let mock_history = ObjectActionHistory {
            uid: hashed_object_id.clone(),
            hashed_sqlite_id: hashed_object_id.clone(),
            latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(1),
            actions: server_actions,
        };

        // We should have 0 actions for this object
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 0);
        });

        // Now manually overwrite the data for this object
        ObjectActions::handle(&app).update(&mut app, |model, ctx| {
            model.overwrite_action_history_for_object(&hashed_object_id, client_actions, ctx)
        });

        // We should have 5 actions for this object
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 5);
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.maybe_overwrite_object_action_history(&mock_history, ctx);
            });

        // Check that the new actions were rejected, and we still have 5 actions
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 5);
        });
    });
}

#[test]
fn test_overwrite_object_action_history_ignores_pending_local_actions() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let timestamp = Utc::now();

        let hashed_object_id = "Notebook-asdf".to_string();
        let hashed_object_id_clone = hashed_object_id.clone();

        let server_actions: Vec<ObjectAction> = vec![
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(1),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(1)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(12),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(12)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];

        // The client actions have one action that is more recent than what the server is sending.
        let client_actions: Vec<ObjectAction> = vec![
            // This action is pending so we should still accept the new
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: true,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(2),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(2)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(10),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(10)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(12),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(12)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp - chrono::Duration::minutes(13),
                    processed_at_timestamp: Some(timestamp - chrono::Duration::minutes(13)),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                uid: hashed_object_id.clone(),
                hashed_sqlite_id: hashed_object_id.clone(),
                action_type: ObjectActionType::Execute,
                action_subtype: ObjectActionSubtype::BundledActions {
                    count: 5,
                    oldest_timestamp: timestamp - chrono::Duration::minutes(35),
                    latest_timestamp: timestamp - chrono::Duration::minutes(15),
                    latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(15),
                },
            },
        ];

        let mock_history = ObjectActionHistory {
            uid: hashed_object_id.clone(),
            hashed_sqlite_id: hashed_object_id.clone(),
            latest_processed_at_timestamp: timestamp - chrono::Duration::minutes(1),
            actions: server_actions,
        };

        ObjectActions::handle(&app).update(&mut app, |model, ctx| {
            model.overwrite_action_history_for_object(&hashed_object_id, client_actions, ctx)
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.maybe_overwrite_object_action_history(&mock_history, ctx);
            });

        // The new actions should be accepted, but the pending action should be persisted as well.
        ObjectActions::handle(&app).update(&mut app, |model, _ctx| {
            assert_eq!(model.count_actions_for_object(&hashed_object_id_clone), 5);
        });
    });
}

#[test]
fn test_object_action_histories_with_initial_load() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();

        let notebook_id_a: SyncId =
            SyncId::ServerId(ServerId::from_string_lossy("JLKS23FSJLKS23FSJLKS23"));
        let notebook_id_b: SyncId =
            SyncId::ServerId(ServerId::from_string_lossy("LSJLK23fZDLSJLK23fZDLS"));
        let notebook_id_c: SyncId =
            SyncId::ServerId(ServerId::from_string_lossy("SDFlJ23SDfSDFlJ23SDfSD"));

        let timestamp = Utc::now();
        let timestamp_old = Utc::now() - chrono::Duration::seconds(1);
        let timestamp_older = Utc::now() - chrono::Duration::seconds(2);
        let timestamp_oldest = Utc::now() - chrono::Duration::seconds(3);

        let actions_a = vec![ObjectAction {
            action_type: ObjectActionType::Execute,
            uid: notebook_id_a.uid(),
            hashed_sqlite_id: notebook_id_a.uid(),
            action_subtype: ObjectActionSubtype::SingleAction {
                timestamp: timestamp_old,
                processed_at_timestamp: Some(timestamp_old),
                data: None,
                pending: false,
            },
        }];
        let actions_b = vec![ObjectAction {
            action_type: ObjectActionType::Execute,
            uid: notebook_id_a.uid(),
            hashed_sqlite_id: notebook_id_a.uid(),
            action_subtype: ObjectActionSubtype::SingleAction {
                timestamp: timestamp_older,
                processed_at_timestamp: Some(timestamp_older),
                data: None,
                pending: false,
            },
        }];

        ObjectActions::handle(&app).update(&mut app, |object_actions, ctx| {
            object_actions.overwrite_action_history_for_object(
                &notebook_id_a.uid(),
                actions_a,
                ctx,
            );

            object_actions.overwrite_action_history_for_object(
                &notebook_id_b.uid(),
                actions_b,
                ctx,
            );
        });

        let actions_a_server = vec![
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: notebook_id_a.uid(),
                hashed_sqlite_id: notebook_id_a.uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: notebook_id_a.uid(),
                hashed_sqlite_id: notebook_id_a.uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp_old,
                    processed_at_timestamp: Some(timestamp_old),
                    data: None,
                    pending: false,
                },
            },
        ];

        let actions_b_server = vec![
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: notebook_id_a.uid(),
                hashed_sqlite_id: notebook_id_a.uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp_old,
                    processed_at_timestamp: Some(timestamp_old),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: notebook_id_a.uid(),
                hashed_sqlite_id: notebook_id_a.uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp: timestamp_older,
                    processed_at_timestamp: Some(timestamp_older),
                    data: None,
                    pending: false,
                },
            },
        ];

        let actions_c_server = vec![ObjectAction {
            action_type: ObjectActionType::Execute,
            uid: notebook_id_a.uid(),
            hashed_sqlite_id: notebook_id_a.uid(),
            action_subtype: ObjectActionSubtype::SingleAction {
                timestamp: timestamp_oldest,
                processed_at_timestamp: Some(timestamp_oldest),
                data: None,
                pending: false,
            },
        }];

        let server_action_histories = vec![
            ObjectActionHistory {
                uid: notebook_id_a.uid(),
                hashed_sqlite_id: notebook_id_a.uid(),
                latest_processed_at_timestamp: timestamp,
                actions: actions_a_server,
            },
            ObjectActionHistory {
                uid: notebook_id_b.uid(),
                hashed_sqlite_id: notebook_id_b.uid(),
                latest_processed_at_timestamp: timestamp_old,
                actions: actions_b_server,
            },
            ObjectActionHistory {
                uid: notebook_id_c.uid(),
                hashed_sqlite_id: notebook_id_c.uid(),
                latest_processed_at_timestamp: timestamp_oldest,
                actions: actions_c_server,
            },
        ];
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let mocked_response = InitialLoadResponse {
            updated_notebooks: vec![],
            deleted_notebooks: vec![],
            updated_folders: vec![],
            deleted_folders: vec![],
            user_profiles: vec![],
            updated_generic_string_objects: Default::default(),
            deleted_generic_string_objects: Default::default(),
            action_histories: server_action_histories,
        };
        receive_initial_load_or_polling_update(
            &mut app,
            &update_manager_struct.update_manager,
            false, /* force_refresh */
            mocked_response,
        );

        // Assert new ObjectAction state
        ObjectActions::handle(&app).update(&mut app, |object_actions, _| {
            assert_eq!(
                object_actions.count_actions_for_object(&notebook_id_a.uid()),
                2
            );
            assert_eq!(
                object_actions.count_actions_for_object(&notebook_id_b.uid()),
                2
            );
            assert_eq!(
                object_actions.count_actions_for_object(&notebook_id_c.uid()),
                1
            );
        });

        let events = db_events(&update_manager_struct);

        // All the upserts from polling
        assert!(matches!(&events[0], ModelEvent::SyncObjectActions { .. }));
        assert!(matches!(&events[1], ModelEvent::UpsertNotebooks(_)));
        assert!(matches!(&events[2], ModelEvent::UpsertFolders(_)));
        assert!(matches!(&events[3], ModelEvent::DeleteObjects { ids: _ }));
    });
}

#[test]
fn test_delete_single_object_with_actions() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        // Create notebook object
        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let ts = Utc::now();

        let server_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: ts.into(),
            trashed_ts: Some(ts.into()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let server_notebook =
            mock_server_notebook(notebook_id, Owner::mock_current_user(), server_metadata);
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(server_notebook.clone()),
            );
        });

        let timestamp = Utc::now();
        // Add some actions to this object
        let actions_on_object = vec![
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: SyncId::ServerId(notebook_id.into()).uid(),
                hashed_sqlite_id: SyncId::ServerId(notebook_id.into()).uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: SyncId::ServerId(notebook_id.into()).uid(),
                hashed_sqlite_id: SyncId::ServerId(notebook_id.into()).uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
            ObjectAction {
                action_type: ObjectActionType::Execute,
                uid: SyncId::ServerId(notebook_id.into()).uid(),
                hashed_sqlite_id: SyncId::ServerId(notebook_id.into()).uid(),
                action_subtype: ObjectActionSubtype::SingleAction {
                    timestamp,
                    processed_at_timestamp: Some(timestamp),
                    data: None,
                    pending: false,
                },
            },
        ];
        ObjectActions::handle(&app).update(&mut app, |object_actions, ctx| {
            object_actions.overwrite_action_history_for_object(
                &SyncId::ServerId(notebook_id.into()).uid(),
                actions_on_object,
                ctx,
            )
        });

        // Mock delete
        server_api
            .expect_delete_object()
            .times(1)
            .return_once(move |_| {
                Ok(ObjectDeleteResult::Success {
                    deleted_ids: vec![SyncId::ServerId(notebook_id.into())],
                })
            });

        // Delete notebook
        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.delete_object_by_user(
                    CloudObjectTypeAndId::Notebook(SyncId::ServerId(notebook_id.into())),
                    ctx,
                );
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // Check that notebook is not in CloudModel anymore
        CloudModel::handle(&app).update(&mut app, |cloud_model, _ctx| {
            assert!(
                !cloud_model.check_if_object_is_in_cloudmodel(notebook_id.to_server_id().uid()),
                "Deleted object should not be in CloudModel anymore"
            );
        });

        // Ensure the actions are also deleted.
        ObjectActions::handle(&app).update(&mut app, |object_actions, _| {
            assert_eq!(
                object_actions
                    .count_actions_for_object(&SyncId::ServerId(notebook_id.into()).uid()),
                0
            );
        });

        let events = db_events(&update_manager_struct);
        assert!(matches!(&events[0], ModelEvent::DeleteObjects { .. }));
    });
}

/// Test successfully moving an object from a user's personal space to a team drive. This covers
/// changing the owner of an object, which implicitly also clears its parent folder.
#[test]
fn test_move_object_personal_to_team_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();
        let team = Space::Team {
            team_uid: ServerId::from(789),
        };

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_transfer_notebook_owner()
            .times(1)
            .return_once(move |_, _| Ok(true));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));
        assert_space_for_object(&app, &sync_id.uid(), Space::Personal);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from a personal folder to the team space.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Space(team),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_space_for_object(&app, &sync_id.uid(), team);
        // Currently, we can only move to the root of another space.
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_id.into()),
                to_folder: None,
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);
        assert_space_for_object(&app, &sync_id.uid(), team);

        // After the move succeeds on the server, we update the DB, but don't emit another model
        // event.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ModelEvent::UpsertNotebook { notebook } if notebook.id == sync_id),
            "Expected upsert of notebook, got {:?}",
            &events[0]
        );
    });
}

/// Test failing to move an object from a user's personal space to a team drive. This tests that we
/// optimistically update the model, and then roll those updates back on failure.
#[test]
fn test_move_object_personal_to_team_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();
        let team = Space::Team {
            team_uid: ServerId::from(789),
        };

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_transfer_notebook_owner()
            .returning(move |_, _| Err(anyhow::anyhow!("move failed")));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));
        assert_space_for_object(&app, &sync_id.uid(), Space::Personal);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from a personal folder to the team space.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Space(team),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_space_for_object(&app, &sync_id.uid(), team);
        // Currently, we can only move to the root of another space.
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_id.into()),
                to_folder: None,
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to fail.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // await long enough that all the move object retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);

        // We should roll back the optimistic update and emit a corresponding event.
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));
        assert_space_for_object(&app, &sync_id.uid(), Space::Personal);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: None,
                to_folder: Some(folder_id.into())
            }]
        );

        // There's no database update to roll back.
        assert!(db_events(&update_manager_struct).is_empty());
    });
}

#[test]
fn test_move_object_root_to_folder_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .times(1)
            .return_once(move |_, _, _, _| Ok(true));

        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from the root to a folder.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Folder(folder_id.into()),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: None,
                to_folder: Some(folder_id.into()),
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        // After the move succeeds on the server, we update the DB, but don't emit another model
        // event.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ModelEvent::UpsertNotebook { notebook } if notebook.id == sync_id),
            "Expected upsert of notebook, got {:?}",
            &events[0]
        );
    });
}

/// Test failing to move an object from the root of a space into a folder. This checks that we
/// optimistically update the model, and then roll back the updates on failure.
#[test]
fn test_move_object_root_to_folder_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .returning(move |_, _, _, _| Err(anyhow::anyhow!("move failed")));

        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from the root to a folder.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Folder(folder_id.into()),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: None,
                to_folder: Some(folder_id.into()),
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to fail.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // await long enough that all the move object retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);

        // We should roll back the optimistic update and emit a corresponding event.
        assert_folder_for_object(&app, &sync_id.uid(), None);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_id.into()),
                to_folder: None
            }]
        );

        // There's no database update to roll back.
        assert!(db_events(&update_manager_struct).is_empty());
    });
}

/// Test moving an object from a folder to the root of its space. This checks metadata before,
/// during, and after a move where the object's parent folder changes from `Some` to `None`.
#[test]
fn test_move_object_folder_to_root_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .times(1)
            .return_once(move |_, _, _, _| Ok(true));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from a folder to the root.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Space(Space::Personal),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_id.into()),
                to_folder: None,
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        // After the move succeeds on the server, we update the DB, but don't emit another model
        // event.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ModelEvent::UpsertNotebook { notebook } if notebook.id == sync_id),
            "Expected upsert of notebook, got {:?}",
            &events[0]
        );
    });
}

/// Test failing to move an object out of a folder and into the root of its space. This checks that
/// we optimistically clear its parent folder, and then restore it on failure.
#[test]
fn test_move_object_folder_to_root_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .returning(move |_, _, _, _| Err(anyhow::anyhow!("move failed")));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from a folder to the root.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Space(Space::Personal),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_root_level_for_object(&mut app, &sync_id.uid(), true);

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_id.into()),
                to_folder: None,
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to fail.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // await long enough that all the move object retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);

        // We should roll back the optimistic update and emit a corresponding event.
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: None,
                to_folder: Some(folder_id.into())
            }]
        );

        // There's no database update to roll back.
        assert!(db_events(&update_manager_struct).is_empty());
    });
}

/// Test successfully moving an object from one folder to another, within the same space. This
/// checks that we update the object's metadata and emit events correctly.
#[test]
fn test_move_object_folder_to_folder_success() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_a_id: FolderId = 456.into();
        let folder_b_id: FolderId = 789.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_a_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_a_id, Owner::mock_current_user(), cloud_model);
            add_folder(folder_b_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .times(1)
            .return_once(move |_, _, _, _| Ok(true));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_a_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from folder A to folder B.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Folder(folder_b_id.into()),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_b_id.into()));

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_a_id.into()),
                to_folder: Some(folder_b_id.into())
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_b_id.into()));

        // After the move succeeds on the server, we update the DB, but don't emit another model
        // event.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ModelEvent::UpsertNotebook { notebook } if notebook.id == sync_id),
            "Expected upsert of notebook, got {:?}",
            &events[0]
        );
    });
}

/// Test failing to move an object from one folder to another in the same space. This checks that
/// we optimistically apply the move in-memory and then undo it on failure.
#[test]
fn test_move_object_folder_to_folder_failure() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_a_id: FolderId = 456.into();
        let folder_b_id: FolderId = 789.into();

        let notebook_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: Some(folder_a_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
            add_folder(folder_a_id, Owner::mock_current_user(), cloud_model);
            add_folder(folder_b_id, Owner::mock_current_user(), cloud_model);
        });

        server_api
            .expect_move_object()
            .returning(move |_, _, _, _| Err(anyhow::anyhow!("move failed")));

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_a_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Move the notebook from folder A to folder B.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.move_object_to_location(
                    type_and_id,
                    CloudObjectLocation::Folder(folder_b_id.into()),
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), true);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_b_id.into()));

        // There should be an optimistic model event, but no database write.
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_a_id.into()),
                to_folder: Some(folder_b_id.into())
            }]
        );
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the move to fail.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // await long enough that all the move object retries are exhausted
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_pending_online_only_change_for_object(&mut app, &sync_id.uid(), false);

        // We should roll back the optimistic update and emit a corresponding event.
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_a_id.into()));
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Local,
                from_folder: Some(folder_b_id.into()),
                to_folder: Some(folder_a_id.into())
            }]
        );

        // There's no database update to roll back.
        assert!(db_events(&update_manager_struct).is_empty());
    });
}

/// Tests that the cloud model is updated correctly if we receive an RTC message indicating that an
/// object was trashed by another client.
#[test]
fn test_trash_object_over_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let current_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(mock_server_notebook(
                    notebook_id,
                    Owner::mock_current_user(),
                    current_metadata,
                )),
            );
        });

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        let new_metadata = ServerMetadata {
            uid: notebook_id.into(),
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: Some(new_metadata_ts.into()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: new_metadata,
            },
        );

        // The metadata changes should be applied in-memory and to the database.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectTrashed {
                type_and_id,
                source: UpdateSource::Server
            }]
        );
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id, .. } if id == &sync_id.sqlite_uid_hash(ObjectIdType::Notebook)
        ));
    });
}

/// Tests that the cloud model is updated correctly if we receive an RTC message indicating that an
/// object was un-trashed by another client.
#[test]
fn test_untrash_object_over_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let current_metadata_ts = Utc::now();
        let current_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: Some(current_metadata_ts.into()),
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(mock_server_notebook(
                    notebook_id,
                    Owner::mock_current_user(),
                    current_metadata,
                )),
            );
        });

        assert_trashed_status_for_object(&mut app, &sync_id.uid(), true);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        let new_metadata = ServerMetadata {
            uid: notebook_id.into(),
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: new_metadata,
            },
        );

        // The metadata changes should be applied in-memory and to the database.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_trashed_status_for_object(&mut app, &sync_id.uid(), false);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectUntrashed {
                type_and_id,
                source: UpdateSource::Server
            }]
        );
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id, .. } if id == &sync_id.sqlite_uid_hash(ObjectIdType::Notebook)
        ));
    });
}

/// Tests that the cloud model is correctly updated if we receive an RTC message indicating that an
/// object was moved from one folder to another in the same space by another client.
#[test]
fn test_move_object_from_folder_to_folder_over_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_a_id: FolderId = 456.into();
        let folder_b_id: FolderId = 789.into();

        let current_metadata_ts = Utc::now();
        let current_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: Some(folder_a_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(mock_server_notebook(
                    notebook_id,
                    Owner::mock_current_user(),
                    current_metadata,
                )),
            );
        });

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_a_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        let new_metadata = ServerMetadata {
            uid: notebook_id.into(),
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: Some(folder_b_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: new_metadata,
            },
        );

        // The metadata changes should be applied in-memory and to the database.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_b_id.into()));
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Server,
                from_folder: Some(folder_a_id.into()),
                to_folder: Some(folder_b_id.into()),
            }]
        );
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id, .. } if id == &sync_id.sqlite_uid_hash(ObjectIdType::Notebook)
        ));
    });
}

/// Tests that the cloud model is updated correctly if we receive an RTC message indicating that an
/// object was moved from a folder to the root of its space by another client.
#[test]
fn test_move_object_from_folder_to_root_over_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let current_metadata_ts = Utc::now();
        let current_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(mock_server_notebook(
                    notebook_id,
                    Owner::mock_current_user(),
                    current_metadata,
                )),
            );
        });

        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        let new_metadata = ServerMetadata {
            uid: notebook_id.into(),
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: new_metadata,
            },
        );

        // The metadata changes should be applied in-memory and to the database.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_folder_for_object(&app, &sync_id.uid(), None);
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Server,
                from_folder: Some(folder_id.into()),
                to_folder: None,
            }]
        );
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id, .. } if id == &sync_id.sqlite_uid_hash(ObjectIdType::Notebook)
        ));
    });
}

/// Tests that we update the cloud model correctly after receiving an RTC message indicating that
/// an object was moved from the root of its space into a folder by another client.
#[test]
fn test_move_object_from_root_to_folder_over_rtc() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let folder_id: FolderId = 456.into();

        let current_metadata_ts = Utc::now();
        let current_metadata = ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: current_metadata_ts.into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };

        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(mock_server_notebook(
                    notebook_id,
                    Owner::mock_current_user(),
                    current_metadata,
                )),
            );
        });

        assert_folder_for_object(&app, &sync_id.uid(), None);

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        let new_metadata_ts = current_metadata_ts + chrono::Duration::seconds(1);
        let new_metadata = ServerMetadata {
            uid: notebook_id.into(),
            revision: Revision::now(),
            metadata_last_updated_ts: new_metadata_ts.into(),
            trashed_ts: None,
            folder_id: Some(folder_id),
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: new_metadata,
            },
        );

        // The metadata changes should be applied in-memory and to the database.
        let type_and_id = CloudObjectTypeAndId::from_id_and_type(sync_id, ObjectType::Notebook);
        assert_folder_for_object(&app, &sync_id.uid(), Some(folder_id.into()));
        assert_eq!(
            cloud_events(&update_manager_struct),
            vec![CloudModelEvent::ObjectMoved {
                type_and_id,
                source: UpdateSource::Server,
                from_folder: None,
                to_folder: Some(folder_id.into()),
            }]
        );
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ModelEvent::UpdateObjectMetadata { id, .. } if id == &sync_id.sqlite_uid_hash(ObjectIdType::Notebook)
        ));
    });
}

#[test]
fn test_permissions_update_grants_access() {
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let mut server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());
        let owner = Owner::Team {
            team_uid: ServerId::from(99),
        };

        let guest_user_id = UserUid::new("abc123");
        let other_guests = vec![UserProfileWithUID {
            firebase_uid: guest_user_id,
            display_name: Some("Warp User".to_string()),
            email: "user@warp.dev".to_string(),
            photo_url: String::new(),
        }];

        let server_permissions = mock_server_permissions(owner);
        let server_notebook = mock_server_notebook(
            notebook_id,
            owner,
            ServerMetadata {
                uid: notebook_id.into(),
                revision: Revision::now(),
                metadata_last_updated_ts: Utc::now().into(),
                trashed_ts: None,
                folder_id: None,
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                current_editor_uid: None,
            },
        );
        let notebook_data = server_notebook.model.data.clone();

        server_api
            .expect_fetch_single_cloud_object()
            .times(1)
            .return_once(move |_| {
                Ok(GetCloudObjectResponse {
                    object: ServerCloudObject::Notebook(server_notebook),
                    action_histories: vec![ObjectActionHistory {
                        uid: notebook_id.to_hash(),
                        hashed_sqlite_id: notebook_id
                            .to_server_id()
                            .sqlite_type_and_uid_hash(ObjectIdType::Notebook),
                        latest_processed_at_timestamp: Utc::now(),
                        actions: vec![],
                    }],
                    descendants: vec![],
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectPermissionsChangedV2 {
                object_uid: notebook_id.into(),
                permissions: server_permissions,
                user_profiles: other_guests.clone(),
            },
        );

        // The object isn't in memory yet.
        assert!(CloudModel::handle(&app).read(&app, |cloud_model, _| {
            cloud_model.get_notebook(&sync_id).is_none()
        }),);
        assert!(cloud_events(&update_manager_struct).is_empty());

        // The permissions change should also insert new user profiles.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ModelEvent::UpsertUserProfiles { profiles } if profiles == &other_guests)
        );
        UserProfiles::handle(&app).read(&app, |user_profiles, _| {
            assert!(user_profiles.profile_for_uid(guest_user_id).is_some());
        });

        // Wait for the fetch to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        // The object should have been fetched.
        assert_notebook_data(&app, sync_id, &notebook_data);

        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ModelEvent::UpsertNotebook { .. }));
        assert!(matches!(&events[1], ModelEvent::SyncObjectActions { .. }));

        let events = cloud_events(&update_manager_struct);
        assert_eq!(
            events,
            vec![CloudModelEvent::ObjectCreated {
                type_and_id: CloudObjectTypeAndId::Notebook(sync_id),
            }]
        );
    });
}

#[test]
fn test_permissions_update_existing_object() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);

    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);

        let server_api = mock_server_api();
        let notebook_id: NotebookId = 123.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        // Model the object already existing in memory.
        let original_server_notebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            ServerMetadata {
                uid: notebook_id.into(),
                revision: Revision::now(),
                metadata_last_updated_ts: Utc::now().into(),
                trashed_ts: None,
                folder_id: None,
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                current_editor_uid: None,
            },
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(
                sync_id,
                CloudNotebook::new_from_server(original_server_notebook),
            );
        });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        assert_space_for_object(&app, &sync_id.uid(), Space::Personal);

        // Receive an RTC update that moves the object.
        receive_object_update_from_rtc(
            &mut app,
            &update_manager_struct.update_manager,
            ObjectUpdateMessage::ObjectPermissionsChangedV2 {
                object_uid: notebook_id.into(),
                permissions: mock_server_permissions(Owner::Team {
                    team_uid: ServerId::from(99),
                }),
                user_profiles: vec![],
            },
        );

        // The object's space should change.
        assert_space_for_object(&app, &sync_id.uid(), Space::Shared);

        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ModelEvent::UpsertNotebook { .. }));

        // We don't currently emit CloudModel events for permission changes - should we?
    });
}

#[test]
fn test_add_guest_success() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let uid = server_id.uid();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        let updated_permissions_ts = Utc::now() + chrono::Duration::seconds(5);
        let updated_permissions = ServerPermissions {
            space: Owner::mock_current_user(),
            guests: vec![ServerObjectGuest {
                subject: ServerGuestSubject::User {
                    firebase_uid: "guest".to_string(),
                },
                access_level: AccessLevel::Editor,
                source: None,
            }],
            anyone_link_sharing: None,
            permissions_last_updated_ts: updated_permissions_ts.into(),
        };

        server_api
            .expect_add_object_guests()
            .times(1)
            .return_once(move |_, _, _| {
                Ok(ObjectPermissionsUpdateData {
                    permissions: updated_permissions,
                    profiles: vec![UserProfileWithUID {
                        firebase_uid: UserUid::new("guest"),
                        display_name: Some("Guest User".to_string()),
                        email: "guest@warp.dev".to_string(),
                        photo_url: "http://example.com".to_string(),
                    }],
                })
            });

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Add the user as a guest.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.add_object_guests(
                    server_id,
                    vec!["guest@warp.dev".to_string()],
                    AccessLevel::Editor,
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &uid, true);

        // There should be no optimistic update.
        assert_eq!(cloud_events(&update_manager_struct), vec![]);
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the permissions change to complete.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;

        assert_pending_online_only_change_for_object(&mut app, &uid, false);
        CloudModel::handle(&app).read(&app, |cloud_model, _| {
            let object = cloud_model.get_by_uid(&uid).expect("Object should exist");
            assert_eq!(
                object.permissions().guests,
                vec![CloudObjectGuest {
                    subject: Subject::User(UserKind::Account(UserUid::new("guest"))),
                    access_level: SharingAccessLevel::Edit,
                    source: None,
                }]
            )
        });

        // After the update succeeds on the server, we update the DB.
        let events = db_events(&update_manager_struct);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], ModelEvent::UpsertNotebook { notebook } if notebook.id == sync_id),
            "Expected upsert of notebook, got {:?}",
            &events[0]
        );
        assert!(matches!(&events[1], ModelEvent::UpsertUserProfiles { .. }));
    });
}

#[test]
fn test_add_guest_failure() {
    let _guard = FeatureFlag::SharedWithMe.override_enabled(true);
    App::test(ASSETS, |mut app| async move {
        initialize_app(&mut app);
        let mut server_api = mock_server_api();

        let server_id: ServerId = 123.into();
        let uid = server_id.uid();
        let notebook_id: NotebookId = server_id.into();
        let sync_id = SyncId::ServerId(notebook_id.into());

        let notebook_metadata = ServerMetadata {
            uid: server_id,
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        };
        let notebook: ServerNotebook = mock_server_notebook(
            notebook_id,
            Owner::mock_current_user(),
            notebook_metadata.clone(),
        );
        CloudModel::handle(&app).update(&mut app, |cloud_model, _| {
            cloud_model.add_object(sync_id, CloudNotebook::new_from_server(notebook));
        });

        server_api
            .expect_add_object_guests()
            .returning(move |_, _, _| Err(anyhow::anyhow!("adding guest failed")));

        let update_manager_struct = create_update_manager_struct(&mut app, Arc::new(server_api));

        // Add the user as a guest.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                update_manager.add_object_guests(
                    server_id,
                    vec!["guest@warp.dev".to_string()],
                    AccessLevel::Editor,
                    ctx,
                );
            });

        assert_pending_online_only_change_for_object(&mut app, &uid, true);

        // There should be no optimistic update.
        assert_eq!(cloud_events(&update_manager_struct), vec![]);
        assert!(db_events(&update_manager_struct).is_empty());

        // Wait for the permissions change and all retries to fail.
        update_manager_struct
            .update_manager
            .update(&mut app, |update_manager, ctx| {
                ctx.await_spawned_future(update_manager.spawned_futures[0])
            })
            .await;
        warpui::r#async::Timer::after(Duration::from_secs(10)).await;

        assert_pending_online_only_change_for_object(&mut app, &uid, false);

        // There should no changes to roll back.
        CloudModel::handle(&app).read(&app, |cloud_model, _| {
            let object = cloud_model.get_by_uid(&uid).expect("Object should exist");
            assert_eq!(object.permissions().guests, vec![])
        });
        assert_eq!(cloud_events(&update_manager_struct), vec![]);
        assert!(db_events(&update_manager_struct).is_empty());
    });
}
