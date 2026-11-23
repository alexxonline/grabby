use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static DOTENV: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

pub fn data_dir() -> &'static Path {
    DATA_DIR.get_or_init(|| {
        #[cfg(test)]
        let dir = std::env::temp_dir().join(format!("grabby-test-{}", std::process::id()));
        #[cfg(not(test))]
        let dir = std::env::var_os("GRABBY_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Grabby")))
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share/grabby")))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

#[cfg(test)]
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

pub fn db_path() -> PathBuf {
    data_dir().join("grabby.db")
}

pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

pub fn env_path() -> PathBuf {
    data_dir().join(".env")
}

fn dotenv() -> &'static RwLock<HashMap<String, String>> {
    DOTENV.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn load_env() {
    let mut values = HashMap::new();
    for path in [env_path(), PathBuf::from(".env")] {
        if let Ok(iter) = dotenvy::from_path_iter(&path) {
            for (key, value) in iter.flatten() {
                values.entry(key).or_insert(value);
            }
        }
    }
    *dotenv().write().unwrap_or_else(|e| e.into_inner()) = values;
}

pub fn env_var(name: &str) -> Option<String> {
    if crate::secrets::NAMES.contains(&name) {
        return crate::secrets::get(name).or_else(|| std::env::var(name).ok().filter(|v| !v.is_empty()));
    }
    if let Ok(v) = std::env::var(name) {
        return Some(v);
    }
    dotenv().read().unwrap_or_else(|e| e.into_inner()).get(name).cloned()
}

pub fn from_process_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty())
}

pub fn remove_env_file_keys(path: &Path, keys: &[String]) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path)?;
    let mut changed = false;
    let lines: Vec<String> = existing
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let key = body.split('=').next().map(str::trim).unwrap_or("");
            if body.contains('=') && keys.iter().any(|k| k == key) {
                changed = true;
                format!("# {key} moved to configs.dat (encrypted with Windows DPAPI); manage it in Grabby > Settings > API keys")
            } else {
                l.to_owned()
            }
        })
        .collect();
    if !changed {
        return Ok(());
    }
    let tmp = path.with_extension("env.tmp");
    std::fs::write(&tmp, lines.join("\n") + "\n")?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_secret_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "A=1\nELEVENLABS_API_KEY=sk_secret\nexport OPENROUTER_API_KEY=or\n").unwrap();
        remove_env_file_keys(&path, &["ELEVENLABS_API_KEY".into(), "OPENROUTER_API_KEY".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sk_secret") && !text.contains("=or"));
        assert!(text.starts_with("A=1\n# ELEVENLABS_API_KEY moved"));
        let parsed: HashMap<_, _> = dotenvy::from_path_iter(&path).unwrap().flatten().collect();
        assert_eq!(parsed.len(), 1);
    }
}
