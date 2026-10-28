// secret.rs - seals text (a WireGuard config) so it can sit in settings.json without exposing keys.
// Windows: DPAPI in user scope, so only the same Windows user on the same machine can open it.
// Other platforms: base64 only, which is NOT encryption. That path exists so the Docker leak lab
// (Linux, headless) can run the same code; never rely on it for real secrets.
// Sealed text is "<tag>:<base64>", tag "dpapi" or "plain". `open` never echoes its input.

use base64::{engine::general_purpose::STANDARD, Engine};

const DPAPI_TAG: &str = "dpapi:";
const PLAIN_TAG: &str = "plain:";

/// Seals `plain`. Returns the tagged base64 text, or an empty string if the OS refused to encrypt
/// (callers must treat empty as failure: falling back to plain text would put keys on disk).
pub fn seal(plain: &str) -> String {
    #[cfg(windows)]
    {
        match dpapi::protect(plain.as_bytes()) {
            Some(blob) => format!("{DPAPI_TAG}{}", STANDARD.encode(blob)),
            None => String::new(),
        }
    }
    #[cfg(not(windows))]
    {
        // Lab-only: no OS key store here, so this is encoding, not encryption.
        format!("{PLAIN_TAG}{}", STANDARD.encode(plain.as_bytes()))
    }
}

/// Opens text produced by `seal`. Errors are generic on purpose: they never include the input.
pub fn open(sealed: &str) -> Result<String, String> {
    if let Some(b64) = sealed.strip_prefix(DPAPI_TAG) {
        let blob = STANDARD.decode(b64).map_err(|_| "sealed profile is damaged (bad base64)".to_string())?;
        #[cfg(windows)]
        {
            let bytes = dpapi::unprotect(&blob)
                .ok_or("could not decrypt the profile (it was sealed by another Windows user or machine)")?;
            return String::from_utf8(bytes).map_err(|_| "decrypted profile is not valid text".to_string());
        }
        #[cfg(not(windows))]
        {
            let _ = blob;
            return Err("this profile was sealed with Windows DPAPI and cannot be opened here".into());
        }
    }
    if let Some(b64) = sealed.strip_prefix(PLAIN_TAG) {
        let bytes = STANDARD.decode(b64).map_err(|_| "sealed profile is damaged (bad base64)".to_string())?;
        return String::from_utf8(bytes).map_err(|_| "sealed profile is not valid text".to_string());
    }
    Err("unknown sealed-profile format (expected a dpapi: or plain: prefix)".into())
}

/// Thin safe wrappers over CryptProtectData / CryptUnprotectData.
#[cfg(windows)]
mod dpapi {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    /// Runs one of the two DPAPI calls over `data` and copies the result out, freeing the OS buffer.
    /// `protect` picks the direction. Flags 0 = current-user scope, no UI prompt is requested.
    fn run(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: `input` points at `data`, which outlives the call and is only read. `out` is a
        // valid blob the OS fills with a LocalAlloc'd buffer; all optional pointers are null.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, null(), null(), null(), null(), 0, &mut out)
            } else {
                CryptUnprotectData(&input, null_mut(), null(), null(), null(), 0, &mut out)
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        // SAFETY: on success `out.pbData` is a buffer of `out.cbData` bytes allocated by the OS;
        // we copy it, then release it with LocalFree exactly once, as the API requires.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        unsafe { LocalFree(out.pbData as _) };
        Some(bytes)
    }

    /// Encrypts for the current Windows user.
    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, true)
    }

    /// Decrypts a blob made by `protect` under the same user.
    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";

    #[test]
    fn seal_then_open_roundtrips() {
        let plain = format!("[Interface]\nPrivateKey = {KEY}\n");
        let sealed = seal(&plain);
        assert!(sealed.starts_with(if cfg!(windows) { "dpapi:" } else { "plain:" }), "{sealed}");
        assert_eq!(open(&sealed).unwrap(), plain);
    }

    #[test]
    fn sealed_text_does_not_contain_the_plain_key() {
        let sealed = seal(&format!("PrivateKey = {KEY}"));
        assert!(!sealed.contains(KEY));
        assert!(!sealed.contains("PrivateKey"));
    }

    #[test]
    fn open_rejects_unknown_prefix_without_echoing_input() {
        let e = open("rot13:SECRET-DO-NOT-ECHO").unwrap_err();
        assert!(e.contains("unknown"), "{e}");
        assert!(!e.contains("SECRET"), "{e}");
        assert!(open("").is_err());
    }

    #[test]
    fn open_rejects_damaged_payload() {
        assert!(open("plain:!!!not base64!!!").is_err());
        assert!(open("dpapi:!!!not base64!!!").is_err());
    }
}
