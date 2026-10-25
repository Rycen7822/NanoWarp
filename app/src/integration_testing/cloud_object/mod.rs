mod assertion;

use std::future::Future;
use std::pin::Pin;

pub use assertion::*;
use futures::FutureExt;
use futures::future::join_all;
use itertools::Itertools;
use warpui::integration::TestStep;
use warpui::{App, SingletonEntity, async_assert};

use crate::cloud_object::Space;
use crate::cloud_object::model::persistence::CloudModel;
use crate::server::cloud_objects::update_manager::{InitiatedBy, UpdateManager};
use crate::server::ids::{ClientId, SyncId};
use crate::workspaces::user_workspaces::UserWorkspaces;

/// Clears the cloud model of all non-welcome objects in the user's personal space.
/// Returns a future that resolves when the cloud model is cleared.
pub fn clear_cloud_model(app: &mut App) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    let object_ids_to_delete = CloudModel::handle(app).read(app, |cloud_model, ctx| {
        cloud_model
            .active_non_welcome_cloud_objects_in_space(Space::Personal, ctx)
            .map(|object| object.cloud_object_type_and_id())
            .collect_vec()
    });

    let mut futures = Vec::new();
    for object_id in object_ids_to_delete {
        UpdateManager::handle(app).update(app, |update_manager, ctx| {
            update_manager.delete_object_by_user(object_id, ctx);
            if let Some(future_id) = update_manager.spawned_futures().last() {
                let future = ctx.await_spawned_future(*future_id);
                futures.push(future);
            }
        });
    }

    Box::pin(join_all(futures).map(|_| ()))
}

/// Create a personal folder and save its sync ID into the step data.
pub fn create_a_personal_folder(key: impl Into<String>, title: impl Into<String>) -> TestStep {
    let key = key.into();
    let title = title.into();
    TestStep::new("Create a personal folder")
        .with_action(move |app, _, data| {
            let client_id = ClientId::new();
            let sync_id = SyncId::ClientId(client_id);
            UpdateManager::handle(app).update(app, |manager, ctx| {
                manager.create_folder(
                    title.clone(),
                    UserWorkspaces::as_ref(ctx)
                        .personal_drive(ctx)
                        .expect("User UID must be set in tests"),
                    client_id,
                    None,
                    true,
                    InitiatedBy::User,
                    ctx,
                );
            });
            data.insert(key.clone(), sync_id);
        })
        .add_assertion(move |app, _| {
            CloudModel::handle(app).read(app, |model, ctx| {
                async_assert!(
                    model
                        .active_cloud_objects_in_space(Space::Personal, ctx)
                        .count()
                        > 0,
                    "Folder exists"
                )
            })
        })
}
