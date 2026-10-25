mod envelope;
mod secret_value;

pub use envelope::{UploadKey, init as init_envelope};
/// Supplies the authenticated actor identity for managed secret encryption clients.
pub trait ActorProvider: Send + Sync + 'static {
    fn actor_uid(&self) -> Option<String>;
}
pub use secret_value::ManagedSecretValue;
