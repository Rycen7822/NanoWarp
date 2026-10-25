use std::collections::HashSet;
use std::ops::Index;
use std::sync::Arc;

use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use cloud_object_client::MockObjectClient;
use cloud_objects::cloud_object::ServerPermissions;
use firebase::FirebaseError;
use itertools::Itertools;
use settings::{RespectUserSyncSetting, SyncToCloud};
use warpui::{App, Entity, ModelHandle, SingletonEntity};

use super::QueueDependency;
use crate::cloud_object::model::actions::{
    ObjectAction, ObjectActionHistory, ObjectActionSubtype, ObjectActionType,
};
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::{
    CloudModelType, CloudObjectEventEntrypoint, CreateCloudObjectResult, CreatedCloudObject,
    GenericStringObjectFormat, JsonObjectType, ObjectIdType, ObjectType, Owner, Revision,
    RevisionAndLastEditor, ServerCreationInfo, UpdateCloudObjectResult,
};
use crate::drive::CloudObjectTypeAndId;
use crate::drive::folders::CloudFolderModel;
use crate::server::cloud_objects::update_manager::InitiatedBy;
use crate::server::ids::{ClientId, HashableId, ServerId, ServerIdAndType, SyncId};
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::auth::UserAuthenticationError;
use crate::server::sync_queue::{
    CreationFailureReason, GenericStringObjectToCreate, QueueItemId, SerializedModel,
    SyncQueueEvent,
};
use crate::settings::{CloudPreferenceModel, Preference};
use crate::system::SystemStats;
use crate::test_util::assert_eventually;
use crate::{NetworkStatus, QueueItem, SyncQueue};

#[derive(Default)]
struct Events(Vec<SyncQueueEvent>);

impl Entity for Events {
    type Event = ();
}

impl Index<usize> for Events {
    type Output = SyncQueueEvent;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
}

fn queue_item_dependencies<const N: usize>(
    queue_item_ids: [QueueItemId; N],
) -> HashSet<QueueDependency> {
    queue_item_ids
        .into_iter()
        .map(QueueDependency::QueueItem)
        .collect()
}

fn bulk_create_dependency(queue_item_id: QueueItemId, client_id: ClientId) -> QueueDependency {
    QueueDependency::BulkCreateGenericStringObject {
        queue_item_id,
        client_id,
    }
}

fn create_sync_queue(
    app: &mut App,
    queue_items: Vec<QueueItem>,
    cloud_objects_client_mock: MockObjectClient,
    immediately_start_dequeueing: bool,
) -> ModelHandle<SyncQueue> {
    let sync_queue = app
        .add_singleton_model(|_| SyncQueue::new(queue_items, Arc::new(cloud_objects_client_mock)));

    if immediately_start_dequeueing {
        SyncQueue::handle(app).update(app, |sync_queue, ctx| {
            sync_queue.start_dequeueing(ctx);
        });
    }

    sync_queue
}

#[test]
fn test_create_folder() {
    App::test((), |mut app| async move {
        let folder_id = ClientId::default();
        let folder_server_id = ServerId::from(1);
        let folder_ts = DateTime::<Utc>::default();
        let folder = CloudFolderModel::new("Shared Folder", false);

        let mut cloud_objects_client_mock = MockObjectClient::new();
        cloud_objects_client_mock
            .expect_create_folder()
            .times(1)
            .return_once(move |_| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id: folder_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: folder_ts.into(),
                            last_editor_uid: None,
                        },
                        metadata_ts: folder_ts.into(),
                        server_id_and_type: ServerIdAndType {
                            id: folder_server_id,
                            id_type: ObjectIdType::Folder,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });

        initialize_app(&mut app);
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, true);

        let sync_queue_events = app.add_model(|_ctx| Events::default());

        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        // Enqueue a single item and wait for the response to complete.
        sync_queue
            .update(&mut app, |sync_queue, ctx| {
                sync_queue.enqueue(
                    QueueItem::CreateObject {
                        object_type: ObjectType::Folder,
                        owner: Owner::mock_current_user(),
                        id: folder_id,
                        title: None,
                        serialized_model: Some(Arc::new(folder.serialized())),
                        initial_folder_id: None,
                        entrypoint: Default::default(),
                        initiated_by: InitiatedBy::User,
                    },
                    ctx,
                );

                ctx.await_spawned_future(sync_queue.spawned_futures[0])
            })
            .await;

        sync_queue_events.update(&mut app, |sync_queue_events, _ctx| {
            assert_eq!(sync_queue_events.0.len(), 1);
            assert_eq!(
                sync_queue_events[0],
                SyncQueueEvent::ObjectCreationSuccessful {
                    server_creation_info: ServerCreationInfo {
                        server_id_and_type: ServerIdAndType {
                            id: folder_server_id,
                            id_type: ObjectIdType::Folder
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                    client_id: folder_id,
                    revision_and_editor: RevisionAndLastEditor {
                        revision: folder_ts.into(),
                        last_editor_uid: None
                    },
                    metadata_ts: folder_ts.into(),
                    initiated_by: InitiatedBy::User
                }
            );
        })
    });
}

#[test]
fn test_generic_string_object_unique_key_failure() {
    App::test((), |mut app| async move {
        let owner = Owner::mock_current_user();
        let gso_id = ClientId::default();
        let gso_json = "{\"storage_key\":\"somepref\",\"value\":true,\"platform\":\"Global\"}";
        let folder_id = ClientId::default();
        let folder_server_id = ServerId::from(1);
        let folder_ts = DateTime::<Utc>::default();
        let folder = CloudFolderModel::new("Shared Folder", false);

        // This pattern is used in a couple places to control the order of async operations.
        // Basically, we wait to receive a message on a channel before certain mocks can continue,
        // so that we have time to assert intermediate states, at which point we fire a message.
        let (tx, rx) = std::sync::mpsc::channel();

        initialize_app(&mut app);

        let mut cloud_objects_client_mock = MockObjectClient::new();
        cloud_objects_client_mock
            .expect_create_generic_string_object()
            .times(1)
            .return_once(move |_, _, _| {
                Ok(CreateCloudObjectResult::GenericStringObjectUniqueKeyConflict)
            });
        cloud_objects_client_mock
            .expect_create_folder()
            .times(1)
            .return_once(move |_| {
                rx.recv().unwrap();
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id: folder_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: folder_ts.into(),
                            last_editor_uid: None,
                        },
                        metadata_ts: folder_ts.into(),
                        server_id_and_type: ServerIdAndType {
                            id: folder_server_id,
                            id_type: ObjectIdType::Folder,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });

        let sync_queue = create_sync_queue(
            &mut app,
            vec![
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner,
                    id: gso_id,
                    title: None,
                    serialized_model: Some(Arc::new(gso_json.to_string().into())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    owner,
                    id: folder_id,
                    title: None,
                    serialized_model: Some(Arc::new(folder.serialized())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
            ],
            cloud_objects_client_mock,
            false,
        );

        let sync_queue_events = app.add_model(|_ctx| Events::default());
        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        // Await the first future.
        sync_queue
            .update(&mut app, |queue, ctx| {
                queue.start_dequeueing(ctx);
                ctx.await_spawned_future(queue.spawned_futures[0])
            })
            .await;

        app.read(|ctx| {
            // The GSO failure should not stop the queue.
            assert!(sync_queue.as_ref(ctx).is_dequeueing());

            let events = sync_queue_events.as_ref(ctx);
            assert_eq!(events.0.len(), 1);
            assert_eq!(
                &events.0,
                &[SyncQueueEvent::ObjectCreationFailure {
                    reason: CreationFailureReason::UniqueKeyConflict {
                        id: gso_id.to_hash(),
                        initiated_by: InitiatedBy::User
                    },
                },]
            );
        });

        // Allow the folder to be created.
        tx.send(()).unwrap();
        sync_queue
            .update(&mut app, |queue, ctx| {
                assert!(queue.is_dequeueing());
                ctx.await_spawned_future(queue.spawned_futures[1])
            })
            .await;

        app.read(|ctx| {
            // The queue should still be dequeueing.
            assert!(sync_queue.as_ref(ctx).is_dequeueing());

            let events = sync_queue_events.as_ref(ctx);
            assert_eq!(events.0.len(), 2);
            assert_eq!(
                &events.0,
                &[
                    SyncQueueEvent::ObjectCreationFailure {
                        reason: CreationFailureReason::UniqueKeyConflict {
                            id: gso_id.to_hash(),
                            initiated_by: InitiatedBy::User
                        },
                    },
                    SyncQueueEvent::ObjectCreationSuccessful {
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: folder_server_id,
                                id_type: ObjectIdType::Folder
                            },
                            creator_uid: None,
                            permissions: ServerPermissions::mock_personal(),
                        },
                        client_id: folder_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: folder_ts.into(),
                            last_editor_uid: None
                        },
                        metadata_ts: folder_ts.into(),
                        initiated_by: InitiatedBy::User
                    }
                ]
            );
        });
    });
}

#[test]
fn test_dequeue_after_transient_failure() {
    App::test((), |mut app| async move {
        let owner = Owner::mock_current_user();
        let success_folder_id = ClientId::default();
        let success_folder_server_id = ServerId::from(1);
        let success_folder_ts = DateTime::<Utc>::default();
        let success_folder = CloudFolderModel::new("Successful Folder", false);
        let failure_folder_id = ClientId::default();
        let failure_folder = CloudFolderModel::new("Failed Folder", false);

        let (tx, rx) = std::sync::mpsc::channel();

        let failure_attempts = 1 + super::DEFAULT_RETRY_OPTION.remaining_retries();
        let mut cloud_objects_client_mock = MockObjectClient::new();
        cloud_objects_client_mock
            .expect_create_folder()
            .times(failure_attempts)
            .returning(move |_| Err(anyhow!("Transient network error!!!!")));
        cloud_objects_client_mock
            .expect_create_folder()
            .times(1)
            .return_once(move |_| {
                rx.recv().unwrap();
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id: success_folder_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: success_folder_ts.into(),
                            last_editor_uid: None,
                        },
                        metadata_ts: success_folder_ts.into(),
                        server_id_and_type: ServerIdAndType {
                            id: success_folder_server_id,
                            id_type: ObjectIdType::Folder,
                        },
                        creator_uid: None,
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });

        initialize_app(&mut app);

        let sync_queue = create_sync_queue(
            &mut app,
            vec![
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    owner,
                    id: failure_folder_id,
                    title: None,
                    serialized_model: Some(Arc::new(failure_folder.serialized())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    owner,
                    id: success_folder_id,
                    title: None,
                    serialized_model: Some(Arc::new(success_folder.serialized())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
            ],
            cloud_objects_client_mock,
            false,
        );

        let sync_queue_events = app.add_model(|_ctx| Events::default());
        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        sync_queue
            .update(&mut app, |queue, ctx| {
                queue.start_dequeueing(ctx);
                ctx.await_spawned_future(queue.spawned_futures[0])
            })
            .await;

        // Wait for the first folder's creation to fail.
        // The failures are retried, but their futures are spawned on background threads,
        // so we can't access them. Instead, we wait for a SyncQueue event to appear in the model.
        assert_eventually!(
            4000 => sync_queue_events.read(&app, |events, _ctx| !events.0.is_empty()),
            "Timed out waiting for failure"
        );

        sync_queue_events.read(&app, |events, _| {
            assert_eq!(
                &events.0[0],
                &SyncQueueEvent::ObjectCreationFailure {
                    reason: CreationFailureReason::Other {
                        id: failure_folder_id.to_hash(),
                        initiated_by: InitiatedBy::User
                    },
                }
            );
        });

        // Allow the second folder to be created.
        tx.send(()).unwrap();
        sync_queue
            .update(&mut app, |queue, ctx| {
                assert!(queue.is_dequeueing());
                ctx.await_spawned_future(queue.spawned_futures[1])
            })
            .await;

        sync_queue_events.read(&app, |events, _| {
            assert_eq!(events.0.len(), 2);
            assert_eq!(
                &events.0,
                &[
                    SyncQueueEvent::ObjectCreationFailure {
                        reason: CreationFailureReason::Other {
                            id: failure_folder_id.to_hash(),
                            initiated_by: InitiatedBy::User
                        },
                    },
                    SyncQueueEvent::ObjectCreationSuccessful {
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: success_folder_server_id,
                                id_type: ObjectIdType::Folder
                            },
                            creator_uid: None,
                            permissions: ServerPermissions::mock_personal(),
                        },
                        client_id: success_folder_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: success_folder_ts.into(),
                            last_editor_uid: None
                        },
                        metadata_ts: success_folder_ts.into(),
                        initiated_by: InitiatedBy::User
                    }
                ]
            );
        });
    });
}

#[test]
fn test_no_dequeue_after_intransient_failure() {
    App::test((), |mut app| async move {
        let owner = Owner::mock_current_user();
        let failure_folder_id = ClientId::default();
        let failure_folder = CloudFolderModel::new("Failed Folder", false);
        let second_folder = CloudFolderModel::new("Second Folder", false);
        let second_folder_id = ClientId::default();
        let second_folder_item = QueueItem::CreateObject {
            object_type: ObjectType::Folder,
            owner,
            id: second_folder_id,
            title: None,
            serialized_model: Some(Arc::new(second_folder.serialized())),
            initial_folder_id: None,
            entrypoint: CloudObjectEventEntrypoint::Unknown,
            initiated_by: InitiatedBy::User,
        };

        let failure_attempts = 1 + super::DEFAULT_RETRY_OPTION.remaining_retries();
        let mut cloud_objects_client_mock = MockObjectClient::new();
        cloud_objects_client_mock
            .expect_create_folder()
            // Note that even though we don't dequeue future items because we know they'll fail,
            // we're still stuck retrying the initial item.
            .times(failure_attempts)
            .returning(move |_| {
                // This is one of the types of errors that won't cause us to keep dequeueing;
                // if Firebase rejects the user once, they'll likely reject requests for other queue items.
                Err(UserAuthenticationError::DeniedAccessToken(FirebaseError {
                    code: 401,
                    message: "Unauthenticated".to_string(),
                })
                .into())
            });

        initialize_app(&mut app);

        let sync_queue = create_sync_queue(
            &mut app,
            vec![
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    owner,
                    id: failure_folder_id,
                    title: None,
                    serialized_model: Some(Arc::new(failure_folder.serialized())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                second_folder_item.clone(),
            ],
            cloud_objects_client_mock,
            false,
        );

        let sync_queue_events = app.add_model(|_ctx| Events::default());
        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        sync_queue
            .update(&mut app, |queue, ctx| {
                queue.start_dequeueing(ctx);
                ctx.await_spawned_future(queue.spawned_futures[0])
            })
            .await;

        // Wait for the first folder's creation to fail.
        // The failures are retried, but their futures are spawned on background threads,
        // so we can't access them. Instead, we wait for a SyncQueue event to appear in the model.
        assert_eventually!(
            4000 => sync_queue_events.read(&app, |events, _ctx| !events.0.is_empty()),
            "Timed out waiting for failure"
        );

        sync_queue_events.read(&app, |events, _| {
            assert_eq!(
                &events.0[0],
                &SyncQueueEvent::ObjectCreationFailure {
                    reason: CreationFailureReason::Other {
                        id: failure_folder_id.to_hash(),
                        initiated_by: InitiatedBy::User
                    },
                }
            );
        });

        // The second folder should not be dequeued.
        sync_queue.read(&app, |queue, _ctx| {
            // The queue will still be in a "dequeueing" state, so it processes new items, but it
            // will not process the existing item.
            assert_eq!(queue.spawned_futures.len(), 1);
            let items = queue.queue().iter().map(|(_, item)| item).collect_vec();
            assert_eq!(items, &[&second_folder_item]);
        });
    });
}

#[test]
fn test_create_and_update_cloud_preference() {
    App::test((), |mut app| async move {
        let owner = Owner::mock_current_user();
        let preference_id = ClientId::default();
        let preference_server_id = ServerId::from(1);
        let preference_ts = DateTime::<Utc>::default();
        let preference_revision_after_create = Revision::from(preference_ts);
        let preference_revision_after_update =
            Revision::from(preference_ts + chrono::Duration::minutes(100));
        let preference = CloudPreferenceModel::new(
            Preference::new(
                "test_storage_key".to_string(),
                "true",
                SyncToCloud::Globally(RespectUserSyncSetting::Yes),
            )
            .expect("error creating preference"),
        );

        let (tx, rx) = std::sync::mpsc::channel();

        let mut cloud_objects_client_mock = MockObjectClient::new();
        let preference_revision_after_create_clone = preference_revision_after_create;
        let preference_revision_after_update_clone = preference_revision_after_update;
        cloud_objects_client_mock
            .expect_create_generic_string_object()
            .times(1)
            .return_once(move |_, _, _| {
                Ok(CreateCloudObjectResult::Success {
                    created_cloud_object: CreatedCloudObject {
                        client_id: preference_id,
                        revision_and_editor: RevisionAndLastEditor {
                            revision: preference_revision_after_create_clone,
                            last_editor_uid: Some("34jkaosdfj".to_string()),
                        },
                        metadata_ts: preference_ts.into(),
                        server_id_and_type: ServerIdAndType {
                            id: preference_server_id,
                            id_type: ObjectIdType::GenericStringObject,
                        },
                        creator_uid: Some("34jkaosdfj".to_string()),
                        permissions: ServerPermissions::mock_personal(),
                    },
                })
            });
        cloud_objects_client_mock
            .expect_update_generic_string_object()
            .times(1)
            .return_once(move |_, _, _| {
                rx.recv().unwrap();
                Ok(UpdateCloudObjectResult::Success {
                    revision_and_editor: RevisionAndLastEditor {
                        revision: preference_revision_after_update_clone,
                        last_editor_uid: Some("34jkaosdfj".to_string()),
                    },
                })
            });

        initialize_app(&mut app);

        let sync_queue = create_sync_queue(
            &mut app,
            vec![
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner,
                    id: preference_id,
                    title: None,
                    serialized_model: Some(Arc::new(preference.serialized())),
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                QueueItem::UpdateCloudPreferences {
                    model: preference.into(),
                    id: SyncId::ClientId(preference_id),
                    revision: Some(preference_revision_after_create),
                },
            ],
            cloud_objects_client_mock,
            true,
        );
        let events = app.add_model(|_ctx| Events::default());

        events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        sync_queue
            .update(&mut app, |queue, ctx| {
                queue.start_dequeueing(ctx);
                ctx.await_spawned_future(queue.spawned_futures[0])
            })
            .await;

        // Assert we have received one event and that it corresponds to a preference creation.
        events.read(&app, |events, _ctx| {
            assert_eq!(events.0.len(), 1);
            assert_eq!(
                events[0],
                SyncQueueEvent::ObjectCreationSuccessful {
                    client_id: preference_id,
                    revision_and_editor: RevisionAndLastEditor {
                        revision: preference_ts.into(),
                        last_editor_uid: Some("34jkaosdfj".to_string()),
                    },
                    metadata_ts: preference_ts.into(),
                    server_creation_info: ServerCreationInfo {
                        server_id_and_type: ServerIdAndType {
                            id: preference_server_id,
                            id_type: ObjectIdType::GenericStringObject,
                        },
                        creator_uid: Some("34jkaosdfj".to_string()),
                        permissions: ServerPermissions::mock_personal(),
                    },
                    initiated_by: InitiatedBy::User
                }
            );
        });

        tx.send(()).unwrap();
        sync_queue
            .update(&mut app, |item, ctx| {
                ctx.await_spawned_future(item.spawned_futures[1])
            })
            .await;

        events.update(&mut app, |sync_queue_events, _ctx| {
            assert_eq!(sync_queue_events.0.len(), 2);
            assert_eq!(
                sync_queue_events[1],
                SyncQueueEvent::ObjectUpdateSuccessful {
                    server_id: preference_server_id,
                    revision_and_editor: RevisionAndLastEditor {
                        revision: preference_revision_after_update,
                        last_editor_uid: Some("34jkaosdfj".to_string()),
                    },
                }
            );
        });

        sync_queue.read(&app, |sync_queue, _ctx| {
            // There should be no more items in the queue.
            assert!(sync_queue.queue.is_empty());
        });
    });
}

// regression test for https://linear.app/warpdotdev/issue/CLD-571
#[test]
fn test_initial_queue_items_processed_properly() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cloud_objects_client_mock = MockObjectClient::new();
        // this is how the sync queue will look when we have 3 pending items from sqlite, but the
        // call to initial load failed, so nothing called `start_dequeueing()`
        let sync_queue = create_sync_queue(
            &mut app,
            vec![
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    serialized_model: Some(Arc::new("folder 1".to_string().into())),
                    owner: Owner::mock_current_user(),
                    id: ClientId::new(),
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    title: None,
                    initiated_by: InitiatedBy::User,
                },
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    serialized_model: Some(Arc::new("folder 2".to_string().into())),
                    owner: Owner::mock_current_user(),
                    id: ClientId::new(),
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    title: None,
                    initiated_by: InitiatedBy::User,
                },
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    serialized_model: Some(Arc::new("folder 3".to_string().into())),
                    owner: Owner::mock_current_user(),
                    id: ClientId::new(),
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    title: None,
                    initiated_by: InitiatedBy::User,
                },
            ],
            cloud_objects_client_mock,
            false,
        );

        sync_queue.update(&mut app, |sync_queue, ctx| {
            sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::Folder,
                    serialized_model: Some(Arc::new("folder 4".to_string().into())),
                    id: ClientId::new(),
                    owner: Owner::mock_current_user(),
                    title: None,
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );
        });

        sync_queue.update(&mut app, |sync_queue, _ctx| {
            // enqueueing a 4th item shouldn't dequeue any items
            assert_eq!(sync_queue.spawned_futures().len(), 0);
            assert_eq!(sync_queue.queue().len(), 4);
        });
    })
}

#[test]
fn test_record_object_action() {
    App::test((), |mut app| async move {
        let timestamp = Utc::now();
        let timestamp_old = timestamp - Duration::minutes(10);
        let timestamp_older = timestamp - Duration::minutes(20);
        let timestamp_oldest = timestamp - Duration::minutes(30);
        let folder_id = "0000watermelonchestnut".to_string();
        let folder_id_clone = folder_id.clone();
        let hashed_sqlite_id = SyncId::ServerId(ServerId::from_string_lossy(&folder_id))
            .sqlite_uid_hash(ObjectIdType::Folder);
        let hashed_sqlite_id_clone = hashed_sqlite_id.clone();

        let mut cloud_objects_client_mock = MockObjectClient::new();
        cloud_objects_client_mock
            .expect_record_object_action()
            .times(1)
            .returning(move |_, _, _, _| {
                Ok(ObjectActionHistory {
                    uid: folder_id_clone.clone(),
                    hashed_sqlite_id: hashed_sqlite_id_clone.clone(),
                    latest_processed_at_timestamp: timestamp,
                    actions: vec![
                        // One action that occurred just now
                        ObjectAction {
                            action_type: ObjectActionType::Execute,
                            uid: folder_id_clone.clone(),
                            hashed_sqlite_id: hashed_sqlite_id_clone.clone(),
                            action_subtype: ObjectActionSubtype::SingleAction {
                                timestamp,
                                processed_at_timestamp: Some(timestamp),
                                data: None,
                                pending: false,
                            },
                        },
                        // One action that occurred a lil bit ago
                        ObjectAction {
                            action_type: ObjectActionType::Execute,
                            uid: folder_id_clone.clone(),
                            hashed_sqlite_id: hashed_sqlite_id_clone.clone(),
                            action_subtype: ObjectActionSubtype::SingleAction {
                                timestamp: timestamp_old,
                                processed_at_timestamp: Some(timestamp_old),
                                data: None,
                                pending: false,
                            },
                        },
                        // A bundle of 10 actions from a lil while ago
                        ObjectAction {
                            action_type: ObjectActionType::Execute,
                            uid: folder_id_clone.clone(),
                            hashed_sqlite_id: hashed_sqlite_id_clone.clone(),
                            action_subtype: ObjectActionSubtype::BundledActions {
                                latest_timestamp: timestamp_older,
                                oldest_timestamp: timestamp_oldest,
                                count: 10,
                                latest_processed_at_timestamp: timestamp_older,
                            },
                        },
                    ],
                })
            });

        initialize_app(&mut app);
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, true);
        let sync_queue_events = app.add_model(|_ctx| Events::default());

        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        // Enqueue a single item and wait for the response to complete.
        sync_queue
            .update(&mut app, |item, ctx| {
                item.enqueue(
                    QueueItem::RecordObjectAction {
                        id_and_type: CloudObjectTypeAndId::Folder(SyncId::ServerId(
                            ServerId::from_string_lossy(&folder_id),
                        )),
                        action_type: ObjectActionType::Execute,
                        action_timestamp: timestamp,
                        data: None,
                    },
                    ctx,
                );

                ctx.await_spawned_future(item.spawned_futures[0])
            })
            .await;

        let uid = SyncId::ServerId(ServerId::from_string_lossy(&folder_id)).uid();
        sync_queue_events.update(&mut app, |sync_queue_events, _ctx| {
            assert_eq!(sync_queue_events.0.len(), 1);
            assert_eq!(
                sync_queue_events[0],
                SyncQueueEvent::ReportObjectActionSucceeded {
                    uid: uid.clone(),
                    action_timestamp: timestamp,
                    action_history: ObjectActionHistory {
                        uid: uid.clone(),
                        hashed_sqlite_id: hashed_sqlite_id.clone(),
                        latest_processed_at_timestamp: timestamp,
                        actions: vec![
                            ObjectAction {
                                action_type: ObjectActionType::Execute,
                                uid: uid.clone(),
                                hashed_sqlite_id: hashed_sqlite_id.clone(),
                                action_subtype: ObjectActionSubtype::SingleAction {
                                    timestamp,
                                    processed_at_timestamp: Some(timestamp),
                                    data: None,
                                    pending: false
                                }
                            },
                            ObjectAction {
                                action_type: ObjectActionType::Execute,
                                uid: uid.clone(),
                                hashed_sqlite_id: hashed_sqlite_id.clone(),
                                action_subtype: ObjectActionSubtype::SingleAction {
                                    timestamp: timestamp_old,
                                    processed_at_timestamp: Some(timestamp_old),
                                    data: None,
                                    pending: false
                                }
                            },
                            ObjectAction {
                                action_type: ObjectActionType::Execute,
                                uid: uid.clone(),
                                hashed_sqlite_id: hashed_sqlite_id.clone(),
                                action_subtype: ObjectActionSubtype::BundledActions {
                                    latest_timestamp: timestamp_older,
                                    oldest_timestamp: timestamp_oldest,
                                    count: 10,
                                    latest_processed_at_timestamp: timestamp_older,
                                }
                            }
                        ]
                    }
                }
            );
        })
    });
}

#[test]
fn test_sync_queue_dependency_successes() {
    // Create a client ID for a preference
    let preference_client_id = ClientId::new();

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let cloud_objects_client_mock = MockObjectClient::new();
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, false);

        sync_queue.update(&mut app, |sync_queue, ctx| {
            // Enqueue the preference create request
            let create_id = sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner: Owner::mock_current_user(),
                    id: preference_client_id,
                    title: None,
                    serialized_model: Some(Arc::new(
                        CloudPreferenceModel::new(
                            Preference::new(
                                "test_storage_key".to_string(),
                                "true",
                                SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                            )
                            .expect("error creating preference"),
                        )
                        .serialized(),
                    )),
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );

            // Enqueue an unrelated request
            sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner: Owner::mock_current_user(),
                    id: ClientId::new(),
                    title: None,
                    serialized_model: None,
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );

            // Enqueue an update to the preference
            let update_id = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "false",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(preference_client_id),
                    revision: None,
                },
                ctx,
            );

            // Assert initial state of the queue dependencies
            assert_eq!(
                sync_queue.queue_dependencies().get(&create_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &queue_item_dependencies([create_id])
            );

            // Simulate success of the create request
            sync_queue.handle_dependency_success(&QueueDependency::QueueItem(create_id));

            // We should no longer store a dependency on the update request
            assert_eq!(
                sync_queue
                    .queue_dependencies()
                    .get(&update_id)
                    .unwrap()
                    .len(),
                0
            )
        });
    })
}

#[test]
fn test_sync_queue_dependency_failure() {
    App::test((), |mut app| async move {
        // have a preference and make the update requests fail
        let client_id = ClientId::new();

        let revision_after_create = Revision::from(DateTime::<Utc>::default());

        let cloud_objects_client_mock = MockObjectClient::new();

        initialize_app(&mut app);

        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, false);

        let sync_queue_events = app.add_model(|_ctx| Events::default());

        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        sync_queue.update(&mut app, |sync_queue, ctx| {
            let create_id = sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner: Owner::mock_current_user(),
                    id: client_id,
                    title: None,
                    serialized_model: None,
                    initial_folder_id: None,
                    entrypoint: Default::default(),
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );

            let update_id = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "true",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(client_id),
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            let update_id_2 = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "false",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(client_id),
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            // Assert initial state of the queue dependencies
            assert_eq!(
                sync_queue.queue_dependencies().get(&create_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &queue_item_dependencies([create_id])
            );
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id_2).unwrap(),
                &queue_item_dependencies([create_id, update_id])
            );

            // Simulate failure of the create request
            sync_queue.remove_id_from_queue(&create_id);
            sync_queue.queue_dependencies.remove(&create_id);
            sync_queue.handle_creation_failure_response(
                client_id.to_string(),
                create_id,
                InitiatedBy::User,
                ctx,
            );

            // We should no longer store any dependencies; they should all have failed and the queue should be empty
            assert!(!sync_queue.queue_dependencies().contains_key(&update_id));
            assert!(!sync_queue.queue_dependencies().contains_key(&update_id_2));
            assert_eq!(sync_queue.queue().len(), 0);
            assert!(sync_queue.queue_dependencies().is_empty());
        });

        sync_queue_events.update(&mut app, |sync_queue_events, _ctx| {
            assert_eq!(sync_queue_events.0.len(), 3); // one creation success, two update failures
            assert_eq!(
                sync_queue_events[0],
                SyncQueueEvent::ObjectUpdateFailure {
                    id: SyncId::ClientId(client_id)
                }
            );
            assert_eq!(
                sync_queue_events[1],
                SyncQueueEvent::ObjectUpdateFailure {
                    id: SyncId::ClientId(client_id)
                },
            );
            assert_eq!(
                sync_queue_events[2],
                SyncQueueEvent::ObjectCreationFailure {
                    reason: CreationFailureReason::Other {
                        id: client_id.to_string(),
                        initiated_by: InitiatedBy::User
                    },
                },
            );
        });
    });
}

#[test]
fn test_sync_queue_dependency_mixed_ids() {
    // check dependencies across items referencing client + server ID for the same object

    let client_id = ClientId::new();
    let server_id = SyncId::ServerId(GenericStringObjectId::from(123).into());
    let revision_after_create = Revision::from(DateTime::<Utc>::default());

    App::test((), |mut app| async move {
        let cloud_objects_client_mock = MockObjectClient::new();
        initialize_app(&mut app);
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, false);
        let sync_queue_events = app.add_model(|_ctx| Events::default());

        sync_queue_events.update(&mut app, |_, ctx| {
            ctx.subscribe_to_model(&sync_queue, |me, _, event, _ctx| me.0.push(event.clone()))
        });

        sync_queue.update(&mut app, |sync_queue, ctx| {
            let create_id = sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner: Owner::mock_current_user(),
                    id: client_id,
                    title: None,
                    serialized_model: None,
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );

            let update_id = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "true",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(client_id),
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            // Assert initial state of the queue dependencies
            assert_eq!(
                sync_queue.queue_dependencies().get(&create_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &queue_item_dependencies([create_id])
            );

            // Simulate success of create request
            sync_queue.remove_id_from_queue(&create_id);
            sync_queue.queue_dependencies.remove(&create_id);
            sync_queue.handle_success_response(
                &server_id.uid(),
                super::ResponseType::Creation {
                    creation_result: super::CreationResponseType::Success {
                        client_id,
                        revision_and_editor: super::RevisionAndLastEditor {
                            revision: revision_after_create,
                            last_editor_uid: None,
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: server_id.into_server().expect("Expect server id"),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            creator_uid: Default::default(),
                            permissions: ServerPermissions::mock_personal(),
                        },
                    },
                },
                create_id,
                InitiatedBy::User,
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );

            let update_id_2 = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "false",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: server_id,
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            // Even though one request uses client ID and one uses server ID, we should find a dependency here
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id_2).unwrap(),
                &queue_item_dependencies([update_id])
            );
        });
    });
}

#[test]
fn test_sync_queue_generic_string_object_update_depends_on_pending_create() {
    let client_id = ClientId::new();
    let server_id = SyncId::ServerId(GenericStringObjectId::from(123).into());
    let revision_after_create = Revision::from(DateTime::<Utc>::default());
    let revision_after_update = Revision::from(DateTime::<Utc>::default() + Duration::minutes(1));

    App::test((), |mut app| async move {
        let cloud_objects_client_mock = MockObjectClient::new();
        initialize_app(&mut app);
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, false);

        sync_queue.update(&mut app, |sync_queue, ctx| {
            let create_id = sync_queue.enqueue(
                QueueItem::CreateObject {
                    object_type: ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                        JsonObjectType::Preference,
                    )),
                    owner: Owner::mock_current_user(),
                    id: client_id,
                    title: None,
                    serialized_model: None,
                    initial_folder_id: None,
                    entrypoint: CloudObjectEventEntrypoint::Unknown,
                    initiated_by: InitiatedBy::User,
                },
                ctx,
            );

            let update_id = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "true",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(client_id),
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&create_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &queue_item_dependencies([create_id])
            );

            sync_queue.remove_id_from_queue(&create_id);
            sync_queue.queue_dependencies.remove(&create_id);
            sync_queue.handle_success_response(
                &server_id.uid(),
                super::ResponseType::Creation {
                    creation_result: super::CreationResponseType::Success {
                        client_id,
                        revision_and_editor: super::RevisionAndLastEditor {
                            revision: revision_after_create,
                            last_editor_uid: None,
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: server_id.into_server().expect("Expect server id"),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            creator_uid: Default::default(),
                            permissions: ServerPermissions::mock_personal(),
                        },
                    },
                },
                create_id,
                InitiatedBy::User,
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );

            let update_id_2 = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "false",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: server_id,
                    revision: Some(revision_after_create),
                },
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id_2).unwrap(),
                &queue_item_dependencies([update_id])
            );

            sync_queue.remove_id_from_queue(&update_id);
            sync_queue.queue_dependencies.remove(&update_id);
            sync_queue.handle_success_response(
                &server_id.uid(),
                super::ResponseType::Update {
                    update_result: super::UpdateResponseType::Success {
                        revision_and_editor: super::RevisionAndLastEditor {
                            revision: revision_after_update,
                            last_editor_uid: None,
                        },
                    },
                },
                update_id,
                InitiatedBy::User,
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_id_2).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            let queued_revision = sync_queue.queue().iter().find_map(|(_, item)| match item {
                QueueItem::UpdateCloudPreferences { id, revision, .. } if *id == server_id => {
                    Some(*revision)
                }
                _ => None,
            });
            assert_eq!(queued_revision, Some(Some(revision_after_update)));
        });
    });
}

#[test]
fn test_sync_queue_bulk_generic_string_object_update_waits_for_matching_create() {
    let client_id_a = ClientId::new();
    let client_id_b = ClientId::new();
    let server_id_a = SyncId::ServerId(GenericStringObjectId::from(123).into());
    let server_id_b = SyncId::ServerId(GenericStringObjectId::from(456).into());
    let revision_after_create_a = Revision::from(DateTime::<Utc>::default());
    let revision_after_create_b = Revision::from(DateTime::<Utc>::default() + Duration::minutes(1));

    App::test((), |mut app| async move {
        let cloud_objects_client_mock = MockObjectClient::new();
        initialize_app(&mut app);
        let sync_queue = create_sync_queue(&mut app, vec![], cloud_objects_client_mock, false);

        sync_queue.update(&mut app, |sync_queue, ctx| {
            let object_to_create = |id| GenericStringObjectToCreate {
                id,
                format: GenericStringObjectFormat::Json(JsonObjectType::Preference),
                serialized_model: Arc::new(SerializedModel::new("{}".to_string())),
                initial_folder_id: None,
                entrypoint: CloudObjectEventEntrypoint::Unknown,
                uniqueness_key: None,
                initiated_by: InitiatedBy::User,
            };

            let bulk_create_id = sync_queue.enqueue(
                QueueItem::BulkCreateGenericStringObjects {
                    owner: Owner::mock_current_user(),
                    objects: vec![object_to_create(client_id_a), object_to_create(client_id_b)],
                },
                ctx,
            );

            // Simulate the state after the bulk create has been dequeued and is waiting
            // on a single server response for both client IDs.
            sync_queue.remove_id_from_queue(&bulk_create_id);
            sync_queue.queue_dependencies.remove(&bulk_create_id);
            sync_queue
                .in_flight_bulk_create_objects
                .insert(bulk_create_id, HashSet::from([client_id_a, client_id_b]));
            for client_id in [client_id_a, client_id_b] {
                sync_queue
                    .waiting_response
                    .entry(client_id.to_string())
                    .or_default()
                    .insert(bulk_create_id);
            }

            let update_b_id = sync_queue.enqueue(
                QueueItem::UpdateCloudPreferences {
                    model: CloudPreferenceModel::new(
                        Preference::new(
                            "test_storage_key".to_string(),
                            "true",
                            SyncToCloud::Globally(RespectUserSyncSetting::Yes),
                        )
                        .expect("error creating preference"),
                    )
                    .into(),
                    id: SyncId::ClientId(client_id_b),
                    revision: Some(revision_after_create_a),
                },
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_b_id).unwrap(),
                &HashSet::from([bulk_create_dependency(bulk_create_id, client_id_b)])
            );

            sync_queue.handle_success_response(
                &server_id_a.uid(),
                super::ResponseType::Creation {
                    creation_result: super::CreationResponseType::Success {
                        client_id: client_id_a,
                        revision_and_editor: super::RevisionAndLastEditor {
                            revision: revision_after_create_a,
                            last_editor_uid: None,
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: server_id_a.into_server().expect("Expect server id"),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            creator_uid: Default::default(),
                            permissions: ServerPermissions::mock_personal(),
                        },
                    },
                },
                bulk_create_id,
                InitiatedBy::User,
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_b_id).unwrap(),
                &HashSet::from([bulk_create_dependency(bulk_create_id, client_id_b)])
            );
            assert_eq!(
                sync_queue
                    .in_flight_bulk_create_objects
                    .get(&bulk_create_id)
                    .unwrap(),
                &HashSet::from([client_id_b])
            );

            sync_queue.handle_success_response(
                &server_id_b.uid(),
                super::ResponseType::Creation {
                    creation_result: super::CreationResponseType::Success {
                        client_id: client_id_b,
                        revision_and_editor: super::RevisionAndLastEditor {
                            revision: revision_after_create_b,
                            last_editor_uid: None,
                        },
                        metadata_ts: DateTime::<Utc>::default().into(),
                        server_creation_info: ServerCreationInfo {
                            server_id_and_type: ServerIdAndType {
                                id: server_id_b.into_server().expect("Expect server id"),
                                id_type: ObjectIdType::GenericStringObject,
                            },
                            creator_uid: Default::default(),
                            permissions: ServerPermissions::mock_personal(),
                        },
                    },
                },
                bulk_create_id,
                InitiatedBy::User,
                ctx,
            );

            assert_eq!(
                sync_queue.queue_dependencies().get(&update_b_id).unwrap(),
                &HashSet::<QueueDependency>::new()
            );
            assert!(
                !sync_queue
                    .in_flight_bulk_create_objects
                    .contains_key(&bulk_create_id)
            );
            let queued_revision = sync_queue.queue().iter().find_map(|(_, item)| match item {
                QueueItem::UpdateCloudPreferences { id, revision, .. }
                    if *id == SyncId::ClientId(client_id_b) =>
                {
                    Some(*revision)
                }
                _ => None,
            });
            assert_eq!(queued_revision, Some(Some(revision_after_create_b)));
        });
    });
}
