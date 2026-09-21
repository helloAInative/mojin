//! 服务端环境文件发现与加载。
//!
//! LaunchAgent 的工作目录通常是 `/`，不能依赖 `dotenv()` 只查当前目录。
//! 优先使用 `MJ_ENV_FILE`，否则从工作目录和可执行文件向上寻找项目根目录。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct EnvStatus {
    pub env_file_loaded: bool,
    pub env_file_path: String,
    pub env_file_error: Option<String>,
}

static ENV_STATUS: OnceLock<EnvStatus> = OnceLock::new();

pub fn load_env() -> &'static EnvStatus {
    ENV_STATUS.get_or_init(load_env_inner)
}

pub fn env_status() -> &'static EnvStatus {
    load_env()
}

fn load_env_inner() -> EnvStatus {
    if let Some(explicit) = std::env::var_os("MJ_ENV_FILE") {
        let path = PathBuf::from(explicit);
        return load_path(path);
    }

    let root = std::env::current_dir()
        .ok()
        .and_then(|path| find_project_root(&path))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| find_project_root(&path))
        });
    let path = root
        .map(|path| path.join(".env"))
        .unwrap_or_else(|| PathBuf::from(".env"));
    load_path(path)
}

fn load_path(path: PathBuf) -> EnvStatus {
    if !path.is_file() {
        return EnvStatus {
            env_file_loaded: false,
            env_file_path: path.display().to_string(),
            env_file_error: None,
        };
    }
    match dotenvy::from_path(&path) {
        Ok(_) => EnvStatus {
            env_file_loaded: true,
            env_file_path: path.display().to_string(),
            env_file_error: None,
        },
        Err(error) => EnvStatus {
            env_file_loaded: false,
            env_file_path: path.display().to_string(),
            env_file_error: Some(error.to_string()),
        },
    }
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    start.ancestors().find_map(|path| {
        (path.join(".env.example").is_file()
            && path.join("mj-server").is_dir()
            && path.join("README.md").is_file())
        .then(|| path.to_path_buf())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_root_is_found_from_server_and_release_paths() {
        let server = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = server.parent().expect("workspace root");
        assert_eq!(find_project_root(server).as_deref(), Some(root));
        assert_eq!(
            find_project_root(&root.join("target/release/mj-server")).as_deref(),
            Some(root)
        );
    }
}
