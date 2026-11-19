//! M10 — Encrypted file vault.
//!
//! A vault is a folder `<Config::dir()>/vaults/<name>/` containing:
//! - `vault.json`: header (format version, Argon2id parameters + salt, the random
//!   32-byte data key (DEK) wrapped with XChaCha20-Poly1305 under the password-derived
//!   key, and the same DEK wrapped under a recovery-key-derived key);
//! - `index.bin`: the encrypted index (file names, sizes, mtimes);
//! - `<id>.blob`: one blob per file = 24-byte random nonce || AEAD ciphertext, with the
//!   file id as associated data (blobs cannot be swapped between ids).
//!
//! No plaintext file name ever touches the container. The password is never stored;
//! a wrong password fails on the AEAD tag of the wrapped DEK (constant-time check in
//! poly1305). Keys and passwords live in `Zeroizing` buffers and are wiped on lock.
//!
//! Argon2id is deliberately slow (~0.5–2 s with the defaults): call `create`/`unlock`/
//! `change_password` from a background thread, not the UI thread.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use halo_core::module::{Category, Manifest, Module, ModuleContext};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "vault",
    cdc: "M10",
    name_key: "module.vault",
    category: Category::Security,
    icon: "lock",
    permissions: &[],
    opt_in: true,
    default_enabled: false,
};

const FORMAT_VERSION: u32 = 1;
const HEADER: &str = "vault.json";
const INDEX: &str = "index.bin";
const NONCE_LEN: usize = 24;
const AAD_PASSWORD: &[u8] = b"halo-vault/v1/dek/password";
const AAD_RECOVERY: &[u8] = b"halo-vault/v1/dek/recovery";
const AAD_INDEX: &[u8] = b"halo-vault/v1/index";
/// Recovery key: 30 base32 chars (150 bits), shown as 6 groups of 5.
const RECOVERY_LEN: usize = 30;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug)]
pub enum VaultError {
    WrongPassword,
    BadRecoveryKey,
    InvalidName,
    NotFound(String),
    AlreadyExists(String),
    Locked,
    /// AEAD verification failed on a blob or the index (modified or corrupted data).
    Tampered(String),
    Corrupt(String),
    Io(io::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPassword => write!(f, "wrong password"),
            Self::BadRecoveryKey => write!(f, "invalid recovery key"),
            Self::InvalidName => write!(f, "invalid vault name"),
            Self::NotFound(s) => write!(f, "not found: {s}"),
            Self::AlreadyExists(s) => write!(f, "already exists: {s}"),
            Self::Locked => write!(f, "vault is locked"),
            Self::Tampered(s) => write!(f, "integrity check failed: {s}"),
            Self::Corrupt(s) => write!(f, "corrupt vault: {s}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<io::Error> for VaultError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, VaultError>;

// ---------------------------------------------------------------------------
// KDF

/// Argon2id cost parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane — the minimum accepted for new vaults.
    pub const MIN: Self = Self { m_cost_kib: 64 * 1024, t_cost: 3, p_cost: 1 };

    /// Raise every cost to at least [`KdfParams::MIN`]; p is always 1.
    pub fn at_least_min(self) -> Self {
        Self { m_cost_kib: self.m_cost_kib.max(Self::MIN.m_cost_kib), t_cost: self.t_cost.max(Self::MIN.t_cost), p_cost: 1 }
    }

    /// Bounds checked on open so a tampered header cannot ask for absurd resources.
    fn sane(&self) -> bool {
        (8..=4 * 1024 * 1024).contains(&self.m_cost_kib) && (1..=64).contains(&self.t_cost) && (1..=16).contains(&self.p_cost)
    }

    fn derive(&self, secret: &[u8], salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        let params = argon2::Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(32))
            .map_err(|e| VaultError::Corrupt(format!("argon2 params: {e}")))?;
        let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut out = Zeroizing::new([0u8; 32]);
        a.hash_password_into(secret, salt, out.as_mut()).map_err(|e| VaultError::Corrupt(format!("argon2: {e}")))?;
        Ok(out)
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::MIN
    }
}

// ---------------------------------------------------------------------------
// Crypto helpers

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS random generator unavailable");
    b
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key).expect("32-byte key")
}

/// nonce || ciphertext+tag
fn seal(key: &[u8; 32], aad: &[u8], msg: &[u8]) -> Vec<u8> {
    let nonce: [u8; NONCE_LEN] = random();
    let ct = cipher(key).encrypt(&nonce.into(), Payload { msg, aad }).expect("XChaCha20-Poly1305 encrypt");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Returns `None` when authentication fails (wrong key, wrong AAD, or modified data).
fn unseal(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if blob.len() < NONCE_LEN + 16 {
        return None;
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    cipher(key).decrypt(&nonce.into(), Payload { msg: ct, aad }).ok().map(Zeroizing::new)
}

fn b64d(s: &str) -> Result<Vec<u8>> {
    B64.decode(s).map_err(|_| VaultError::Corrupt("bad base64 in header".into()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Overwrite a file with random bytes, flush it, truncate and delete it.
///
/// Best effort: on SSDs (wear levelling) and copy-on-write/snapshotted volumes the old
/// blocks may survive; this only defeats casual undelete.
pub fn secure_delete(path: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::metadata(path)
        && meta.is_file() {
            let mut f = fs::OpenOptions::new().write(true).open(path)?;
            let mut left = meta.len();
            let mut chunk = vec![0u8; 64 * 1024];
            while left > 0 {
                let n = left.min(chunk.len() as u64) as usize;
                getrandom::fill(&mut chunk[..n]).map_err(|e| io::Error::other(e.to_string()))?;
                f.write_all(&chunk[..n])?;
                left -= n as u64;
            }
            f.sync_all()?;
            f.set_len(0)?;
            f.sync_all()?;
        }
    fs::remove_file(path)
}

// ---------------------------------------------------------------------------
// Recovery key

/// One-time recovery key, e.g. `ABCDE-FGH2J-...` (6 groups of 5 base32 chars).
/// Show it once to the user; it is never stored.
pub struct RecoveryKey(Zeroizing<String>);

impl RecoveryKey {
    fn generate() -> Self {
        let bytes: Zeroizing<[u8; RECOVERY_LEN]> = Zeroizing::new(random());
        let mut s = Zeroizing::new(String::with_capacity(RECOVERY_LEN + 5));
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 && i % 5 == 0 {
                s.push('-');
            }
            // 256 is a multiple of 32: uniform.
            s.push(B32[(b & 31) as usize] as char);
        }
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Canonical 30-char form used as KDF input; tolerant of case, dashes, spaces and
    /// 0/O, 1/I, 8/B confusions.
    fn normalize(input: &str) -> Option<Zeroizing<String>> {
        let mut out = Zeroizing::new(String::with_capacity(RECOVERY_LEN));
        for c in input.chars() {
            let c = match c.to_ascii_uppercase() {
                '-' | ' ' | '\t' => continue,
                '0' => 'O',
                '1' => 'I',
                '8' => 'B',
                c => c,
            };
            if !B32.contains(&(c as u32 as u8)) || !c.is_ascii() {
                return None;
            }
            out.push(c);
        }
        (out.len() == RECOVERY_LEN).then_some(out)
    }
}

impl fmt::Display for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey(***)")
    }
}

// ---------------------------------------------------------------------------
// On-disk header

#[derive(Serialize, Deserialize)]
struct Wrapped {
    /// Argon2id salt (base64).
    salt: String,
    /// nonce || ciphertext of the DEK (base64).
    dek: String,
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    kdf_alg: String,
    kdf: KdfParams,
    password: Wrapped,
    recovery: Wrapped,
}

impl Header {
    fn read(dir: &Path) -> Result<Self> {
        let text = fs::read_to_string(dir.join(HEADER))?;
        let h: Header = serde_json::from_str(&text).map_err(|e| VaultError::Corrupt(format!("header: {e}")))?;
        if h.version != FORMAT_VERSION || h.kdf_alg != "argon2id" || !h.kdf.sane() {
            return Err(VaultError::Corrupt("unsupported header".into()));
        }
        Ok(h)
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let text = serde_json::to_vec_pretty(self).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        write_atomic(&dir.join(HEADER), &text)?;
        Ok(())
    }
}

fn wrap(kdf: &KdfParams, secret: &[u8], aad: &[u8], dek: &[u8; 32]) -> Result<Wrapped> {
    let salt: [u8; 16] = random();
    let kek = kdf.derive(secret, &salt)?;
    Ok(Wrapped { salt: B64.encode(salt), dek: B64.encode(seal(&kek, aad, dek)) })
}

fn unwrap_dek(kdf: &KdfParams, secret: &[u8], aad: &[u8], w: &Wrapped) -> Result<Option<Zeroizing<[u8; 32]>>> {
    let salt = b64d(&w.salt)?;
    let blob = b64d(&w.dek)?;
    let kek = kdf.derive(secret, &salt)?;
    Ok(unseal(&kek, aad, &blob).and_then(|p| {
        let arr: [u8; 32] = p.as_slice().try_into().ok()?;
        Some(Zeroizing::new(arr))
    }))
}

// ---------------------------------------------------------------------------
// Store and open vault

/// One file stored in a vault (decrypted index entry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Random 128-bit id (hex); also the blob file name.
    pub id: String,
    pub name: String,
    pub size: u64,
    /// Source modification time, unix seconds.
    pub mtime: i64,
    pub added: i64,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.trim() == name
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Where vaults live and which KDF cost new vaults get.
#[derive(Clone, Debug)]
pub struct VaultStore {
    root: PathBuf,
    kdf: KdfParams,
}

impl VaultStore {
    pub fn default_root() -> PathBuf {
        halo_core::config::Config::dir().join("vaults")
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), kdf: KdfParams::MIN }
    }

    /// Tune the KDF cost of new vaults (never below [`KdfParams::MIN`]).
    pub fn with_kdf(mut self, kdf: KdfParams) -> Self {
        self.kdf = kdf.at_least_min();
        self
    }

    /// Cheap KDF for unit tests only.
    #[cfg(test)]
    pub(crate) fn for_tests(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), kdf: KdfParams { m_cost_kib: 64, t_cost: 1, p_cost: 1 } }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn kdf(&self) -> KdfParams {
        self.kdf
    }

    /// Names of the vaults found under the root (sorted).
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join(HEADER).is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        v.sort();
        v
    }

    fn dir(&self, name: &str) -> Result<PathBuf> {
        if !valid_name(name) {
            return Err(VaultError::InvalidName);
        }
        Ok(self.root.join(name))
    }

    /// Create a new empty vault and return it unlocked with its one-time recovery key.
    pub fn create(&self, name: &str, password: &str) -> Result<(OpenVault, RecoveryKey)> {
        let dir = self.dir(name)?;
        if dir.join(HEADER).exists() {
            return Err(VaultError::AlreadyExists(name.into()));
        }
        fs::create_dir_all(&dir)?;
        let dek = Zeroizing::new(random::<32>());
        let recovery = RecoveryKey::generate();
        let canon = RecoveryKey::normalize(recovery.as_str()).expect("generated key is canonical");
        let header = Header {
            version: FORMAT_VERSION,
            kdf_alg: "argon2id".into(),
            kdf: self.kdf,
            password: wrap(&self.kdf, password.as_bytes(), AAD_PASSWORD, &dek)?,
            recovery: wrap(&self.kdf, canon.as_bytes(), AAD_RECOVERY, &dek)?,
        };
        let vault = OpenVault { name: name.into(), dir: dir.clone(), dek, entries: Vec::new(), via_recovery: false };
        vault.save_index()?;
        // Header last: a vault only "exists" once fully initialised.
        header.write(&dir)?;
        Ok((vault, recovery))
    }

    pub fn open(&self, name: &str, password: &str) -> Result<OpenVault> {
        let dir = self.dir(name)?;
        if !dir.join(HEADER).is_file() {
            return Err(VaultError::NotFound(name.into()));
        }
        let h = Header::read(&dir)?;
        let dek = unwrap_dek(&h.kdf, password.as_bytes(), AAD_PASSWORD, &h.password)?.ok_or(VaultError::WrongPassword)?;
        OpenVault::load(name, dir, dek, false)
    }

    pub fn open_with_recovery(&self, name: &str, key: &str) -> Result<OpenVault> {
        let dir = self.dir(name)?;
        if !dir.join(HEADER).is_file() {
            return Err(VaultError::NotFound(name.into()));
        }
        let canon = RecoveryKey::normalize(key).ok_or(VaultError::BadRecoveryKey)?;
        let h = Header::read(&dir)?;
        let dek = unwrap_dek(&h.kdf, canon.as_bytes(), AAD_RECOVERY, &h.recovery)?.ok_or(VaultError::BadRecoveryKey)?;
        OpenVault::load(name, dir, dek, true)
    }
}

/// An unlocked vault. Dropping it wipes the key and the decrypted index.
pub struct OpenVault {
    name: String,
    dir: PathBuf,
    dek: Zeroizing<[u8; 32]>,
    entries: Vec<Entry>,
    via_recovery: bool,
}

impl fmt::Debug for OpenVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenVault").field("name", &self.name).field("files", &self.entries.len()).finish_non_exhaustive()
    }
}

impl Drop for OpenVault {
    fn drop(&mut self) {
        for e in &mut self.entries {
            e.name.zeroize();
        }
    }
}

impl OpenVault {
    fn load(name: &str, dir: PathBuf, dek: Zeroizing<[u8; 32]>, via_recovery: bool) -> Result<Self> {
        let blob = fs::read(dir.join(INDEX))?;
        let plain = unseal(&dek, AAD_INDEX, &blob).ok_or_else(|| VaultError::Tampered("index".into()))?;
        let entries: Vec<Entry> = serde_json::from_slice(&plain).map_err(|e| VaultError::Corrupt(format!("index: {e}")))?;
        Ok(Self { name: name.into(), dir, dek, entries, via_recovery })
    }

    fn save_index(&self) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(&self.entries).map_err(|e| VaultError::Corrupt(e.to_string()))?);
        write_atomic(&self.dir.join(INDEX), &seal(&self.dek, AAD_INDEX, &plain))?;
        Ok(())
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.blob"))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// True when unlocked with the recovery key (then `change_password` needs no old password).
    pub fn unlocked_via_recovery(&self) -> bool {
        self.via_recovery
    }

    pub fn list(&self) -> &[Entry] {
        &self.entries
    }

    /// Encrypt a file into the vault (read fully into memory).
    pub fn add_file(&mut self, path: &Path) -> Result<Entry> {
        let meta = fs::metadata(path)?;
        if !meta.is_file() {
            return Err(VaultError::NotFound(path.display().to_string()));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or(VaultError::InvalidName)?;
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let data = Zeroizing::new(fs::read(path)?);
        self.add_bytes(&name, &data, mtime)
    }

    /// Encrypt an in-memory buffer as a file named `name`.
    pub fn add_bytes(&mut self, name: &str, data: &[u8], mtime: i64) -> Result<Entry> {
        let id: String = random::<16>().iter().map(|b| format!("{b:02x}")).collect();
        write_atomic(&self.blob_path(&id), &seal(&self.dek, id.as_bytes(), data))?;
        let entry = Entry { id: id.clone(), name: name.into(), size: data.len() as u64, mtime, added: now_unix() };
        self.entries.push(entry.clone());
        if let Err(e) = self.save_index() {
            self.entries.pop();
            let _ = fs::remove_file(self.blob_path(&id));
            return Err(e);
        }
        Ok(entry)
    }

    fn entry(&self, id: &str) -> Result<&Entry> {
        self.entries.iter().find(|e| e.id == id).ok_or_else(|| VaultError::NotFound(id.into()))
    }

    /// Decrypt a file into memory.
    pub fn read(&self, id: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.entry(id)?;
        if !valid_id(id) {
            return Err(VaultError::Corrupt("bad id".into()));
        }
        let blob = fs::read(self.blob_path(id))?;
        unseal(&self.dek, id.as_bytes(), &blob).ok_or_else(|| VaultError::Tampered(id.into()))
    }

    /// Decrypt a file into `dest_dir` (never overwrites: adds " (n)" on clash).
    pub fn extract(&self, id: &str, dest_dir: &Path) -> Result<PathBuf> {
        let entry = self.entry(id)?;
        let data = self.read(id)?;
        let base = Path::new(&entry.name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| id.into());
        fs::create_dir_all(dest_dir)?;
        let mut dest = dest_dir.join(&base);
        let mut n = 1;
        while dest.exists() {
            let p = Path::new(&base);
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            dest = dest_dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&dest)?;
        f.write_all(&data)?;
        f.sync_all()?;
        Ok(dest)
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        let pos = self.entries.iter().position(|e| e.id == id).ok_or_else(|| VaultError::NotFound(id.into()))?;
        let removed = self.entries.remove(pos);
        if let Err(e) = self.save_index() {
            self.entries.insert(pos, removed);
            return Err(e);
        }
        let _ = fs::remove_file(self.blob_path(id));
        Ok(())
    }

    /// Re-wrap the data key under a new password (files are not re-encrypted).
    /// `old` is required unless the vault was unlocked with the recovery key.
    pub fn change_password(&mut self, old: Option<&str>, new: &str) -> Result<()> {
        let mut h = Header::read(&self.dir)?;
        if !self.via_recovery {
            let old = old.ok_or(VaultError::WrongPassword)?;
            let dek = unwrap_dek(&h.kdf, old.as_bytes(), AAD_PASSWORD, &h.password)?.ok_or(VaultError::WrongPassword)?;
            if *dek != *self.dek {
                return Err(VaultError::WrongPassword);
            }
        }
        // Password and recovery wraps share the header's KDF params (the recovery wrap
        // cannot be redone without the recovery key), so a fresh salt but the same cost.
        let kdf = h.kdf;
        h.password = wrap(&kdf, new.as_bytes(), AAD_PASSWORD, &self.dek)?;
        h.write(&self.dir)?;
        self.via_recovery = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Password strength

const COMMON: &[&str] = &[
    "123456", "123456789", "12345678", "password", "qwerty", "123123", "111111", "abc123", "1234567", "azerty",
    "000000", "iloveyou", "1234567890", "password1", "qwerty123", "654321", "dragon", "monkey", "letmein", "football",
    "baseball", "sunshine", "princess", "admin", "welcome", "solo", "master", "shadow", "superman", "michael",
    "trustno1", "passw0rd", "starwars", "whatever", "qazwsx", "freedom", "hello", "charlie", "jordan", "soleil",
    "motdepasse", "bonjour", "marseille", "doudou", "chouchou", "loulou", "nicolas", "camille", "123qwe", "zaq12wsx",
    "aaaaaa", "qwertyuiop", "azertyuiop", "1q2w3e4r", "pokemon",
];

/// Local strength estimate: 0 (very weak) ..= 4 (strong). Never leaves the machine.
pub fn password_strength(pw: &str) -> u8 {
    let lower = pw.to_lowercase();
    let len = pw.chars().count();
    if len < 6 || COMMON.iter().any(|c| *c == lower) {
        return 0;
    }
    // A common password with a couple of trailing digits/symbols is still weak.
    let core: String = lower.trim_end_matches(|c: char| !c.is_alphabetic()).into();
    if core.len() >= 4 && COMMON.contains(&core.as_str()) {
        return 1;
    }
    let mut pool = 0u32;
    if pw.chars().any(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if pw.chars().any(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if pw.chars().any(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if pw.chars().any(|c| c.is_ascii() && !c.is_ascii_alphanumeric()) {
        pool += 33;
    }
    if !pw.is_ascii() {
        pool += 100;
    }
    // Penalise repetition and simple sequences: effective length is bounded by the
    // number of distinct chars and by the number of non-sequential steps.
    let chars: Vec<char> = pw.chars().collect();
    let mut distinct = chars.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let steps = chars.windows(2).filter(|w| (w[1] as i64 - w[0] as i64).abs() > 1).count() + 1;
    let effective = len.min(distinct.len() * 2).min(steps * 2) as f64;
    let bits = effective * (pool.max(1) as f64).log2();
    match bits {
        b if b < 28.0 => 0,
        b if b < 40.0 => 1,
        b if b < 60.0 => 2,
        b if b < 80.0 => 3,
        _ => 4,
    }
}

// ---------------------------------------------------------------------------
// Handle (UI side)

struct State {
    store: VaultStore,
    open: Option<OpenVault>,
    last_used: Instant,
    auto_lock: Option<Duration>,
    secure_delete_temp: bool,
    temp_files: Vec<PathBuf>,
    last_error: Option<String>,
}

impl State {
    fn lock_now(&mut self) {
        self.open = None; // Drop wipes the DEK and index.
        if self.secure_delete_temp {
            for p in self.temp_files.drain(..) {
                if let Err(e) = secure_delete(&p)
                    && e.kind() != io::ErrorKind::NotFound {
                        log::warn!("vault: cannot securely delete temp file: {e}");
                    }
                if let Some(parent) = p.parent() {
                    let _ = fs::remove_dir(parent);
                }
            }
        }
    }
}

/// Cloneable handle: list vaults, unlock/lock, manage files, auto-lock.
#[derive(Clone)]
pub struct VaultHandle(Arc<Mutex<State>>);

impl VaultHandle {
    pub fn new(store: VaultStore) -> Self {
        Self(Arc::new(Mutex::new(State {
            store,
            open: None,
            last_used: Instant::now(),
            auto_lock: Some(Duration::from_secs(10 * 60)),
            secure_delete_temp: true,
            temp_files: Vec::new(),
            last_error: None,
        })))
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn record<T>(&self, r: Result<T>) -> Result<T> {
        let mut s = self.st();
        match &r {
            Ok(_) => {
                s.last_error = None;
                s.last_used = Instant::now();
            }
            Err(e) => s.last_error = Some(e.to_string()),
        }
        r
    }

    fn with_open<T>(&self, f: impl FnOnce(&mut OpenVault) -> Result<T>) -> Result<T> {
        let r = {
            let mut s = self.st();
            match s.open.as_mut() {
                Some(v) => f(v),
                None => Err(VaultError::Locked),
            }
        };
        self.record(r)
    }

    pub fn vaults(&self) -> Vec<String> {
        let store = self.st().store.clone();
        store.names()
    }

    /// Name of the unlocked vault, if any.
    pub fn unlocked(&self) -> Option<String> {
        self.st().open.as_ref().map(|v| v.name.clone())
    }

    pub fn last_error(&self) -> Option<String> {
        self.st().last_error.clone()
    }

    /// Create (and unlock) a vault. Slow (Argon2id): call off the UI thread.
    pub fn create(&self, name: &str, password: &str) -> Result<RecoveryKey> {
        let store = self.st().store.clone();
        let r = store.create(name, password).map(|(v, key)| {
            let mut s = self.st();
            s.lock_now();
            s.open = Some(v);
            key
        });
        self.record(r)
    }

    /// Unlock with the password. Slow (Argon2id): call off the UI thread.
    pub fn unlock(&self, name: &str, password: &str) -> Result<()> {
        let store = self.st().store.clone();
        let r = store.open(name, password).map(|v| {
            let mut s = self.st();
            s.lock_now();
            s.open = Some(v);
        });
        self.record(r)
    }

    pub fn unlock_with_recovery(&self, name: &str, key: &str) -> Result<()> {
        let store = self.st().store.clone();
        let r = store.open_with_recovery(name, key).map(|v| {
            let mut s = self.st();
            s.lock_now();
            s.open = Some(v);
        });
        self.record(r)
    }

    /// Wipe keys and decrypted index; securely delete temp extractions.
    pub fn lock(&self) {
        self.st().lock_now();
    }

    pub fn list(&self) -> Vec<Entry> {
        self.st().open.as_ref().map(|v| v.entries.clone()).unwrap_or_default()
    }

    pub fn add_file(&self, path: &Path) -> Result<Entry> {
        self.with_open(|v| v.add_file(path))
    }

    pub fn extract(&self, id: &str, dest_dir: &Path) -> Result<PathBuf> {
        self.with_open(|v| v.extract(id, dest_dir))
    }

    /// Extract into a private temp folder (to open the file); it is securely deleted on lock.
    pub fn extract_temp(&self, id: &str) -> Result<PathBuf> {
        let sub: String = random::<8>().iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("halo-vault-{sub}"));
        let r = self.with_open(|v| v.extract(id, &dir));
        if let Ok(p) = &r {
            self.st().temp_files.push(p.clone());
        }
        r
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.with_open(|v| v.remove(id))
    }

    /// Slow (Argon2id). `old` may be `None` only after a recovery-key unlock.
    pub fn change_password(&self, old: Option<&str>, new: &str) -> Result<()> {
        self.with_open(|v| v.change_password(old, new))
    }

    /// Auto-lock after `minutes` idle (0 = never).
    pub fn set_auto_lock(&self, minutes: u32) {
        self.st().auto_lock = (minutes > 0).then(|| Duration::from_secs(minutes as u64 * 60));
    }

    pub fn set_secure_delete_temp(&self, on: bool) {
        self.st().secure_delete_temp = on;
    }

    /// Mark user activity (resets the idle timer).
    pub fn touch(&self) {
        self.st().last_used = Instant::now();
    }

    /// Auto-lock check; returns `true` if the vault was locked by this call.
    pub fn tick(&self) -> bool {
        self.tick_at(Instant::now())
    }

    fn tick_at(&self, now: Instant) -> bool {
        let mut s = self.st();
        match (s.open.is_some(), s.auto_lock) {
            (true, Some(limit)) if now.saturating_duration_since(s.last_used) >= limit => {
                s.lock_now();
                true
            }
            _ => false,
        }
    }

    pub fn password_strength(pw: &str) -> u8 {
        password_strength(pw)
    }
}

// ---------------------------------------------------------------------------
// Module

pub struct Vault {
    handle: VaultHandle,
    worker: Option<Worker>,
}

impl Vault {
    pub fn new() -> Self {
        Self::with_store(VaultStore::new(VaultStore::default_root()))
    }

    /// Vaults under a custom root (tests, portable mode).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self::with_store(VaultStore::new(root))
    }

    pub fn with_store(store: VaultStore) -> Self {
        Self { handle: VaultHandle::new(store), worker: None }
    }

    pub fn handle(&self) -> VaultHandle {
        self.handle.clone()
    }
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Vault {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let h = self.handle.clone();
        self.worker = Some(Worker::every("halo-vault", Duration::from_secs(15), move || {
            if h.tick() {
                log::info!("vault auto-locked after inactivity");
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
        self.handle.lock();
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.handle.lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("halo-vault-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_open_wrong_password_and_recovery() {
        let root = tmp("open");
        let store = VaultStore::for_tests(&root);
        let (v, key) = store.create("perso", "correct horse").unwrap();
        drop(v);
        assert_eq!(store.names(), vec!["perso".to_string()]);
        assert!(store.open("perso", "correct horse").is_ok());
        assert!(matches!(store.open("perso", "wrong"), Err(VaultError::WrongPassword)));
        assert!(matches!(store.open("nope", "x"), Err(VaultError::NotFound(_))));
        assert!(matches!(store.create("perso", "x"), Err(VaultError::AlreadyExists(_))));
        assert!(matches!(store.create("../evil", "x"), Err(VaultError::InvalidName)));

        // Recovery key: 6 groups of 5, tolerant of case/dashes.
        let k = key.as_str().to_string();
        assert_eq!(k.len(), 35);
        assert_eq!(k.split('-').count(), 6);
        assert!(store.open_with_recovery("perso", &k).is_ok());
        assert!(store.open_with_recovery("perso", &k.replace('-', "").to_lowercase()).is_ok());
        let mut bad = k.clone().into_bytes();
        bad[0] = if bad[0] == b'A' { b'B' } else { b'A' };
        assert!(matches!(store.open_with_recovery("perso", std::str::from_utf8(&bad).unwrap()), Err(VaultError::BadRecoveryKey)));
        assert!(matches!(store.open_with_recovery("perso", "short"), Err(VaultError::BadRecoveryKey)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn change_password() {
        let root = tmp("chpw");
        let store = VaultStore::for_tests(&root);
        let (mut v, key) = store.create("v", "old-pass").unwrap();
        v.add_bytes("a.txt", b"hello", 0).unwrap();
        assert!(matches!(v.change_password(Some("bad"), "new-pass"), Err(VaultError::WrongPassword)));
        assert!(matches!(v.change_password(None, "new-pass"), Err(VaultError::WrongPassword)));
        v.change_password(Some("old-pass"), "new-pass").unwrap();
        drop(v);
        assert!(matches!(store.open("v", "old-pass"), Err(VaultError::WrongPassword)));
        let v = store.open("v", "new-pass").unwrap();
        assert_eq!(v.list().len(), 1);
        drop(v);
        // Recovery still works and allows resetting without the old password.
        let mut v = store.open_with_recovery("v", key.as_str()).unwrap();
        v.change_password(None, "third").unwrap();
        drop(v);
        assert_eq!(&*store.open("v", "third").unwrap().read(&store.open("v", "third").unwrap().list()[0].id.clone()).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn add_extract_roundtrip_remove_and_no_plaintext_names() {
        let root = tmp("files");
        let src = tmp("files-src");
        let out = tmp("files-out");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect();
        let secret_name = "TopSecretBudget2026.xlsx";
        fs::write(src.join(secret_name), &data).unwrap();

        let store = VaultStore::for_tests(&root);
        let (mut v, _) = store.create("work", "pw-pw-pw").unwrap();
        let e = v.add_file(&src.join(secret_name)).unwrap();
        assert_eq!(e.size, data.len() as u64);
        assert_eq!(e.name, secret_name);
        drop(v);

        let mut v = store.open("work", "pw-pw-pw").unwrap();
        assert_eq!(v.list(), &[e.clone()][..]);
        let p = v.extract(&e.id, &out).unwrap();
        assert_eq!(fs::read(&p).unwrap(), data);
        // Second extract does not overwrite.
        let p2 = v.extract(&e.id, &out).unwrap();
        assert_ne!(p, p2);

        // No plaintext file name (or content) anywhere in the container.
        for f in fs::read_dir(root.join("work")).unwrap().flatten() {
            let fname = f.file_name().to_string_lossy().into_owned();
            assert!(!fname.contains("TopSecret"), "{fname}");
            let bytes = fs::read(f.path()).unwrap();
            assert!(!bytes.windows(9).any(|w| w == b"TopSecret"), "plaintext name in {fname}");
            assert!(!bytes.windows(64).any(|w| w == &data[1000..1064]), "plaintext data in {fname}");
        }

        v.remove(&e.id).unwrap();
        assert!(v.list().is_empty());
        assert!(!root.join("work").join(format!("{}.blob", e.id)).exists());
        drop(v);
        assert!(store.open("work", "pw-pw-pw").unwrap().list().is_empty());
        for d in [&root, &src, &out] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn tamper_detection() {
        let root = tmp("tamper");
        let store = VaultStore::for_tests(&root);
        let (mut v, _) = store.create("t", "pw").unwrap();
        let e = v.add_bytes("f.bin", &[7u8; 1000], 0).unwrap();
        let blob = root.join("t").join(format!("{}.blob", e.id));
        let mut b = fs::read(&blob).unwrap();
        b[100] ^= 0x01;
        fs::write(&blob, &b).unwrap();
        assert!(matches!(v.read(&e.id), Err(VaultError::Tampered(_))));
        // Swapping a blob under another id is detected too (id is the AAD).
        b[100] ^= 0x01;
        let e2 = v.add_bytes("g.bin", &[9u8; 10], 0).unwrap();
        fs::write(root.join("t").join(format!("{}.blob", e2.id)), &b).unwrap();
        assert!(matches!(v.read(&e2.id), Err(VaultError::Tampered(_))));
        drop(v);
        // Tampered index.
        let idx = root.join("t").join(INDEX);
        let mut b = fs::read(&idx).unwrap();
        let n = b.len() - 1;
        b[n] ^= 0x80;
        fs::write(&idx, &b).unwrap();
        assert!(matches!(store.open("t", "pw"), Err(VaultError::Tampered(_))));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn handle_lock_autolock_and_temp_secure_delete() {
        let root = tmp("handle");
        let h = VaultHandle::new(VaultStore::for_tests(&root));
        assert!(h.vaults().is_empty());
        let _key = h.create("h", "pass").unwrap();
        assert_eq!(h.unlocked().as_deref(), Some("h"));
        let src = root.join("note.txt");
        fs::write(&src, b"note").unwrap();
        let e = h.add_file(&src).unwrap();
        let tmpfile = h.extract_temp(&e.id).unwrap();
        assert_eq!(fs::read(&tmpfile).unwrap(), b"note");
        h.lock();
        assert!(h.unlocked().is_none());
        assert!(!tmpfile.exists(), "temp extraction securely deleted on lock");
        assert!(matches!(h.add_file(&src), Err(VaultError::Locked)));
        assert!(h.last_error().is_some());
        assert!(h.list().is_empty());

        assert!(h.unlock("h", "nope").is_err());
        h.unlock("h", "pass").unwrap();
        assert!(h.last_error().is_none());
        h.set_auto_lock(1);
        assert!(!h.tick_at(Instant::now() + Duration::from_secs(30)));
        assert!(h.tick_at(Instant::now() + Duration::from_secs(61)));
        assert!(h.unlocked().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn secure_delete_removes_file() {
        let d = tmp("sdel");
        let p = d.join("x.bin");
        fs::write(&p, vec![1u8; 100_000]).unwrap();
        secure_delete(&p).unwrap();
        assert!(!p.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn strength_estimates() {
        assert_eq!(password_strength(""), 0);
        assert_eq!(password_strength("password"), 0);
        assert_eq!(password_strength("Azerty"), 0);
        assert!(password_strength("password123") <= 1);
        assert_eq!(password_strength("aaaaaaaaaaaaaaaa"), 0);
        assert!(password_strength("abcdefghijkl") <= 1);
        assert!(password_strength("Tr0ub4dor&3") >= 2);
        assert_eq!(password_strength("correct horse battery staple"), 4);
        assert_eq!(password_strength("k8#Vq!2zR@m9Lp$w"), 4);
    }

    #[test]
    fn kdf_minimums_enforced() {
        let s = VaultStore::new("x").with_kdf(KdfParams { m_cost_kib: 1024, t_cost: 1, p_cost: 4 });
        assert_eq!(s.kdf(), KdfParams::MIN);
        let s = VaultStore::new("x").with_kdf(KdfParams { m_cost_kib: 128 * 1024, t_cost: 4, p_cost: 1 });
        assert_eq!(s.kdf().m_cost_kib, 128 * 1024);
    }
}
