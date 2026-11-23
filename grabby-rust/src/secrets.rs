use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::config;

pub const ELEVENLABS: &str = "ELEVENLABS_API_KEY";
pub const OPENROUTER: &str = "OPENROUTER_API_KEY";
pub const SYNC: &str = "GRABBY_SYNC_API_KEY";
pub const NAMES: [&str; 3] = [ELEVENLABS, OPENROUTER, SYNC];

static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    secrets: BTreeMap<String, String>,
}

pub fn store_path() -> PathBuf {
    config::data_dir().join("configs.dat")
}

pub fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn entropy(name: &str) -> Vec<u8> {
    format!("grabby:secrets:v1:{name}").into_bytes()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() }
    }

    fn take(out: CRYPT_INTEGER_BLOB, wipe_after: bool) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc.
        let data = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
        let copy = data.to_vec();
        if wipe_after {
            super::wipe(data);
        }
        // SAFETY: the buffer came from DPAPI and is freed exactly once.
        unsafe { LocalFree(out.pbData.cast()) };
        copy
    }

    pub fn protect(plain: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let description: Vec<u16> = "Grabby API key\0".encode_utf16().collect();
        let (input, extra) = (blob(plain), blob(entropy));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: all pointers are valid for the call; DPAPI only reads the inputs.
        let ok = unsafe {
            CryptProtectData(&input, description.as_ptr(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("Windows could not encrypt the key ({}).", std::io::Error::last_os_error()));
        }
        Ok(take(out, false))
    }

    pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let (input, extra) = (blob(sealed), blob(entropy));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: all pointers are valid for the call; DPAPI only reads the inputs.
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("Windows could not decrypt the key ({}).", std::io::Error::last_os_error()));
        }
        Ok(take(out, true))
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_: &[u8], _: &[u8]) -> Result<Vec<u8>, String> {
        Err("Encrypted key storage needs Windows DPAPI.".into())
    }
    pub fn unprotect(_: &[u8], _: &[u8]) -> Result<Vec<u8>, String> {
        Err("Encrypted key storage needs Windows DPAPI.".into())
    }
}

fn read_store(path: &Path) -> Result<Store, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| format!("{} is damaged.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store { version: 1, ..Default::default() }),
        Err(e) => Err(e.to_string()),
    }
}

fn write_store(path: &Path, store: &Store) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("dat.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn get_in(path: &Path, name: &str) -> Result<Option<String>, String> {
    let store = read_store(path)?;
    let Some(encoded) = store.secrets.get(name) else { return Ok(None) };
    let sealed = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| format!("{name} in configs.dat is damaged."))?;
    let mut plain = dpapi::unprotect(&sealed, &entropy(name))?;
    let value = String::from_utf8(plain.clone());
    wipe(&mut plain);
    value.map(Some).map_err(|_| format!("{name} in configs.dat is damaged."))
}

fn set_in(path: &Path, name: &str, value: &str) -> Result<(), String> {
    let sealed = dpapi::protect(value.as_bytes(), &entropy(name))?;
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = read_store(path)?;
    store.version = 1;
    store.secrets.insert(name.to_owned(), base64::engine::general_purpose::STANDARD.encode(sealed));
    write_store(path, &store)
}

fn remove_in(path: &Path, name: &str) -> Result<(), String> {
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = read_store(path)?;
    if store.secrets.remove(name).is_some() {
        write_store(path, &store)?;
    }
    Ok(())
}

pub fn get(name: &str) -> Option<String> {
    match get_in(&store_path(), name) {
        Ok(v) => v.filter(|v| !v.is_empty()),
        Err(e) => {
            eprintln!("secrets: {e}");
            None
        }
    }
}

pub fn is_set(name: &str) -> bool {
    read_store(&store_path()).is_ok_and(|s| s.secrets.contains_key(name))
}

pub fn set(name: &str, value: &str) -> Result<(), String> {
    if !NAMES.contains(&name) {
        return Err(format!("Unknown secret {name}."));
    }
    let value = value.trim();
    if value.contains(['\r', '\n']) || value.chars().any(char::is_control) {
        return Err("Invalid API key.".into());
    }
    if value.is_empty() {
        return remove(name);
    }
    set_in(&store_path(), name, value)
}

pub fn remove(name: &str) -> Result<(), String> {
    remove_in(&store_path(), name)
}

pub fn migrate_from_env_file() {
    let env_path = config::env_path();
    let Ok(iter) = dotenvy::from_path_iter(&env_path) else { return };
    let found: Vec<(String, String)> =
        iter.flatten().filter(|(k, v)| NAMES.contains(&k.as_str()) && !v.trim().is_empty()).collect();
    let mut moved = Vec::new();
    for (name, mut value) in found {
        let ok = is_set(&name) || set(&name, &value).is_ok();
        // SAFETY: wiping a String's bytes with zeros keeps it valid UTF-8.
        wipe(unsafe { value.as_bytes_mut() });
        if ok {
            moved.push(name);
        }
    }
    if !moved.is_empty() {
        if let Err(e) = config::remove_env_file_keys(&env_path, &moved) {
            eprintln!("secrets: keys were encrypted, but {} could not be cleaned: {e}", env_path.display());
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_dpapi() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("configs.dat");
        set_in(&path, ELEVENLABS, "sk_test_123").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("sk_test_123"), "plain text must not reach disk");
        assert_eq!(get_in(&path, ELEVENLABS).unwrap().as_deref(), Some("sk_test_123"));
        let mut store = read_store(&path).unwrap();
        let blob = store.secrets[ELEVENLABS].clone();
        store.secrets.insert(OPENROUTER.into(), blob);
        write_store(&path, &store).unwrap();
        assert!(get_in(&path, OPENROUTER).is_err());
        remove_in(&path, ELEVENLABS).unwrap();
        assert_eq!(get_in(&path, ELEVENLABS).unwrap(), None);
    }
}
