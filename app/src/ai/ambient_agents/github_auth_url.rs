use url::Url;

use crate::ChannelState;

#[derive(Clone, Copy, Debug)]
enum OAuthNextPlatform {
    Native,
    Web,
}

pub fn cloud_setup_auth_url_with_next(base_auth_url: &str) -> String {
    let Ok(mut url) = Url::parse(base_auth_url) else {
        return base_auth_url.to_string();
    };

    let scheme_for_next = std::env::var("WARP_OAUTH_NEXT_SCHEME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            url.query_pairs()
                .find(|(key, _)| key == "scheme")
                .map(|(_, value)| value.into_owned())
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ChannelState::url_scheme().to_string());

    let platform = if cfg!(target_family = "wasm") {
        OAuthNextPlatform::Web
    } else {
        OAuthNextPlatform::Native
    };

    let next_url = build_next_url(&scheme_for_next, platform)
        .unwrap_or_else(|| format!("{scheme_for_next}://action/focus_cloud_mode"));

    let existing_pairs = url
        .query_pairs()
        .filter(|(key, _)| key != "next")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();

    {
        let mut query_pairs = url.query_pairs_mut();
        query_pairs.clear();
        for (key, value) in existing_pairs {
            query_pairs.append_pair(&key, &value);
        }
        query_pairs.append_pair("next", &next_url);
    }

    url.to_string()
}

fn build_next_url(scheme_for_next: &str, platform: OAuthNextPlatform) -> Option<String> {
    match platform {
        OAuthNextPlatform::Native => {
            let base = format!("{scheme_for_next}://action/focus_cloud_mode");
            let mut url = Url::parse(&base).ok()?;

            url.query_pairs_mut()
                .append_pair("source", crate::uri::CLOUD_SETUP_SOURCE);

            Some(url.to_string())
        }
        OAuthNextPlatform::Web => {
            let mut url = Url::parse(&ChannelState::server_root_url()).ok()?;
            url.set_query(None);

            url.set_path("/action/focus_cloud_mode");

            Some(url.to_string())
        }
    }
}
