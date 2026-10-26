use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::AppId;

#[derive(Debug, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// The application ID for this channel.
    pub app_id: AppId,

    /// The name of the file to which logs should be written.
    pub logfile_name: Cow<'static, str>,

    /// HTTP endpoint configuration for font fallback and web links.
    pub server_config: WarpServerConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WarpServerConfig {
    /// The HTTP origin used by font fallback and web-link integrations.
    pub server_root_url: Cow<'static, str>,
}

impl WarpServerConfig {
    pub fn production() -> Self {
        Self {
            server_root_url: "https://app.warp.dev".into(),
        }
    }
}
