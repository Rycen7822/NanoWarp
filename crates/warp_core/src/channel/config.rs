use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::AppId;

#[derive(Debug, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// The application ID for this channel.
    pub app_id: AppId,

    /// The name of the file to which logs should be written.
    pub logfile_name: Cow<'static, str>,

    /// Configuration for talking to Warp's servers.
    pub server_config: WarpServerConfig,
    /// Configuration for telemetry sending, or [`None`] if telemetry should be
    /// disabled for this build.
    pub telemetry_config: Option<TelemetryConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WarpServerConfig {
    /// The root URL for the standard server pool.
    pub server_root_url: Cow<'static, str>,
    /// The URL for the RTC server, which serves real-time updates for Warp Drive objects.
    pub rtc_server_url: Cow<'static, str>,
    /// The URL for the session sharing server, or [`None`] if session sharing is not
    /// supported.
    pub session_sharing_server_url: Option<Cow<'static, str>>,
    /// The API key to use when making requests to Firebase Authentication endpoints.
    pub firebase_auth_api_key: Cow<'static, str>,
}

impl WarpServerConfig {
    pub fn production() -> Self {
        Self {
            server_root_url: "https://app.warp.dev".into(),
            rtc_server_url: "wss://rtc.app.warp.dev/graphql/v2".into(),
            session_sharing_server_url: Some("wss://sessions.app.warp.dev".into()),
            firebase_auth_api_key: "AIzaSyBdy3O3S9hrdayLJxJ7mriBR4qgUaUygAs".into(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelemetryConfig {
    /// The name of the file in which not-yet-sent telemetry events will be stored.
    pub telemetry_file_name: Cow<'static, str>,
    /// Configuration for Rudderstack, for reporting telemetry events.
    pub rudderstack_config: Option<RudderStackConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct RudderStackConfig {
    pub write_key: Cow<'static, str>,
    pub root_url: Cow<'static, str>,
}

impl RudderStackConfig {
    pub fn non_ugc_destination(&self) -> RudderStackDestination {
        RudderStackDestination {
            root_url: self.root_url.clone(),
            write_key: self.write_key.clone(),
        }
    }
}

#[derive(Default)]
pub struct RudderStackDestination {
    pub root_url: Cow<'static, str>,
    pub write_key: Cow<'static, str>,
}
