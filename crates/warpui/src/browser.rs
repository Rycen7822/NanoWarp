#[cfg(any(target_family = "wasm", test))]
pub(crate) fn safe_browser_open_url(url: &str) -> Option<String> {
    let parsed_url = url::Url::parse(url).ok()?;
    match parsed_url.scheme() {
        "http" | "https" | "mailto" | "warp" | "warppreview" | "warpdev" | "warplocal"
        | "warposs" | "warpintegration" => Some(parsed_url.to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
