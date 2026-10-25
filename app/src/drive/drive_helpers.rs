use warp_server_auth::AuthStateProvider;
use warp_server_auth::auth_state::AuthState;
use warpui::{AppContext, SingletonEntity};

use crate::auth::auth_manager::AuthManager;
use crate::cloud_object::Space;
use crate::cloud_object::model::persistence::CloudModel;

pub(crate) fn is_feature_gated_anonymous_user_past_notebook_limit(
    auth_state: &AuthState,
    num_objects: usize,
) -> bool {
    auth_state
        .is_anonymous_user_feature_gated()
        .unwrap_or_default()
        && auth_state
            .personal_object_limits()
            .is_some_and(|limits| num_objects > limits.notebook_limit)
}

pub fn has_feature_gated_anonymous_user_reached_notebook_limit(ctx: &mut AppContext) -> bool {
    let count = CloudModel::handle(ctx).read(ctx, |model, ctx| {
        model
            .active_non_welcome_notebooks_in_space(Space::Personal, ctx)
            .count()
    });
    if AuthStateProvider::handle(ctx).read(ctx, |provider, _| {
        is_feature_gated_anonymous_user_past_notebook_limit(provider.get(), count + 1)
    }) {
        AuthManager::handle(ctx).update(ctx, |manager, ctx| {
            manager.anonymous_user_hit_drive_object_limit(ctx);
        });
        return true;
    }
    false
}
