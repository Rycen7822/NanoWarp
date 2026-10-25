#[cfg(feature = "local_fs")]
pub use crate::persistence::{PersistenceScope, database_file_path_for_scope};

use warpui::async_assert;
use warpui::integration::AssertionCallback;

use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::model::persistence::CloudModel;
use crate::settings::{CloudPreferenceModel, Preference};

pub fn assert_cloud_preference_exists(expected_preference: Preference) -> AssertionCallback {
    Box::new(move |app, _window_id| {
        let stored_preference =
            app.get_singleton_model_handle::<CloudModel>()
                .read(app, |cloud_model, _| {
                    let object = cloud_model
                        .get_all_objects_of_type::<GenericStringObjectId, CloudPreferenceModel>()
                        .find(|p| p.model().string_model == expected_preference)
                        .expect("Expected to find a matching preference object");
                    object.model().string_model.clone()
                });
        async_assert!(
            expected_preference == stored_preference,
            "Expected json object contents to match:\n{expected_preference:?}\nBut got:\n{stored_preference:?}"
        )
    })
}
