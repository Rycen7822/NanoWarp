use cloud_object_persistence::{
    CloudObjectReadContext, id_from_metadata, read_generic_string_object_rows,
    to_cloud_object_metadata,
};
use cloud_objects::cloud_object::{
    GENERIC_STRING_OBJECT_PREFIX, GenericStringObjectFormat, JSON_OBJECT_PREFIX, JsonObjectType,
    ObjectType,
};
use cloud_objects::ids::GenericStringObjectId;
use diesel::SqliteConnection;
use diesel::result::Error;

use crate::{CloudPreference, CloudPreferenceModel};

pub fn read_generic_string_objects(
    conn: &mut SqliteConnection,
    read_context: &CloudObjectReadContext,
) -> Result<Vec<CloudPreference>, Error> {
    Ok(read_generic_string_object_rows(conn)?
        .into_iter()
        .filter_map(|object| {
            let metadata = read_context.metadata_for_object(
                object.id,
                ObjectType::GenericStringObject(GenericStringObjectFormat::Json(
                    JsonObjectType::Preference,
                )),
            )?;
            let object_id = id_from_metadata::<GenericStringObjectId>(metadata)?;
            let cloud_object_permissions = read_context.permissions_for_metadata(metadata)?;
            let JsonObjectType::Preference = metadata
                .object_type
                .strip_prefix(&format!(
                    "{GENERIC_STRING_OBJECT_PREFIX}{JSON_OBJECT_PREFIX}"
                ))?
                .try_into()
                .ok()?;
            let model = CloudPreferenceModel::deserialize_owned(&object.data);
            model.ok().map(|model| {
                CloudPreference::new(
                    object_id,
                    model,
                    to_cloud_object_metadata(metadata),
                    cloud_object_permissions,
                )
            })
        })
        .collect())
}
