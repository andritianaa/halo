//! Per-user encryption at rest via DPAPI (CryptProtectData), used for the optional
//! encrypted clipboard history (M7) and activity log (M4). Keys never leave Windows.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    v
}

pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
    Some(take(out))
}

pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
    Some(take(out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let enc = super::protect(b"secret").unwrap();
        assert_ne!(enc, b"secret");
        assert_eq!(super::unprotect(&enc).unwrap(), b"secret");
        assert!(super::unprotect(b"garbage").is_none());
    }
}
