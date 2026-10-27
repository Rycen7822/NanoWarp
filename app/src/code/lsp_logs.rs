use std::path::{Path, PathBuf};

use lsp::supported_servers::LSPServerType;
use sha2::{Digest, Sha256};

/// Returns the relative log path (within the LSP log directory) for an LSP server.
/// For example, `rust-analyzer/12345678.log`.
pub fn relative_log_path(server_type: LSPServerType, workspace_path: &Path) -> PathBuf {
    let server_type_name = server_type.binary_name();
    let workspace_hash = hash_workspace_path(workspace_path);

    PathBuf::from(server_type_name).join(format!("{workspace_hash}.log"))
}

/// Hashes the workspace path to create a filesystem-safe identifier.
/// Uses first 16 characters of SHA256 hex digest (64 bits of entropy).
fn hash_workspace_path(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    // Take first 8 bytes (16 hex chars) for a shorter but still unique identifier
    hex::encode(&result[..8])
}
