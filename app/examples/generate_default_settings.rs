//! Generates a TOML settings file containing all registered public settings
//! with their default values.
//!
//! Unlike a runtime `SettingsManager` walk, this binary iterates the
//! `inventory::iter::<SettingSchemaEntry>` registry populated by the
//! `define_setting!` / `implement_setting_for_enum!` macros. Any setting
//! defined via those macros is picked up automatically — there is no
//! per-generator registration list to keep in sync.
//!
//! Usage:
//!   cargo run --locked -j2 -p warp --example generate_default_settings -- <output_path>
//!
//! Example:
//!   cargo run --locked -j2 -p warp --example generate_default_settings -- .work/default_settings.toml
//!
//! Defaults follow the Cargo features and debug/release configuration of the compiled OSS build.

use std::path::PathBuf;

use settings::schema::SettingSchemaEntry;
use warp_core::channel::ChannelState;
use warp_core::features::DEBUG_FLAGS;
use warpui_extras::user_preferences::UserPreferences as _;
use warpui_extras::user_preferences::toml_backed::TomlBackedUserPreferences;

/// Ensures all `inventory::submit!` registrations from the app crate's
/// dependency tree are linked into the binary.
///
/// Binary targets only link crate code that is transitively referenced.
/// Without an explicit reference to the `warp` library, the linker will
/// not include most of the app's object files and the `inventory`
/// submissions they contain.
fn ensure_settings_linked() {
    let _ = std::hint::black_box(warp::settings::RESTORE_SESSION);
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let output_path = match (args.next(), args.next()) {
        (Some(path), None) if !path.to_string_lossy().starts_with('-') => PathBuf::from(path),
        _ => {
            eprintln!("Usage: generate_default_settings <output_path>");
            std::process::exit(1);
        }
    };

    let mut state = ChannelState::init();
    if cfg!(debug_assertions) {
        state = state.with_additional_features(DEBUG_FLAGS);
    }
    ChannelState::set(state);
    warp::features::init_feature_flags();
    ensure_settings_linked();

    // Use a new output path for a clean document. Keep the backend's existing
    // handling of pre-existing or invalid files.
    let (toml_prefs, _) = TomlBackedUserPreferences::new(output_path.clone());

    let mut written = 0usize;
    let mut failed = 0usize;

    for entry in inventory::iter::<SettingSchemaEntry> {
        // Skip private settings — they live in the platform-native store and
        // never appear in the user-visible TOML file.
        if entry.is_private {
            continue;
        }

        // Skip settings whose feature flag is not enabled in this build.
        if let Some(flag) = entry.feature_flag
            && !flag.is_enabled()
        {
            continue;
        }

        let default_json = (entry.file_default_value_fn)();

        match toml_prefs.write_value_with_hierarchy(
            entry.storage_key,
            default_json,
            entry.hierarchy,
            entry.max_table_depth,
        ) {
            Err(err) => {
                eprintln!("Warning: failed to write {}: {err}", entry.storage_key);
                failed += 1;
            }
            _ => {
                written += 1;
            }
        }
    }

    println!(
        "Generated default settings at {} ({written} written, {failed} failed)",
        output_path.display()
    );
}
