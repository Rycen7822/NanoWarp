use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use uuid::Uuid;

#[derive(Clone)]
pub struct ShellReplacementRequest {
    pub id: Uuid,
    pub working_directory: PathBuf,
    pub env_vars: HashMap<OsString, OsString>,
}
