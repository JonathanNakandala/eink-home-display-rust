//! Who the server is running as, in the form a security descriptor string names them: the user's SID.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// A process token that is closed when it goes out of scope.
struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `OpenProcessToken` and is closed only here.
        unsafe { CloseHandle(self.0) };
    }
}

/// The SID of the user this process runs as, as text such as `S-1-5-21-…`.
pub fn current_user_sid() -> io::Result<String> {
    let mut handle: HANDLE = ptr::null_mut();
    // SAFETY: `handle` is a valid place for the token to be written, and is wrapped for closing only if it was.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Token(handle);

    // The first call fails by design and says how much room the answer needs.
    let mut needed = 0u32;
    // SAFETY: a null buffer of length 0 is how the size is asked for.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // A `TOKEN_USER` holds a pointer, so the buffer is made of words that are aligned for one.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: the buffer is at least `needed` bytes long.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: the call above filled the buffer with a `TOKEN_USER` whose SID points into the same buffer.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` is valid for as long as `buffer` is, and `text` is a place for the allocated string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `text` points to a null-terminated string that this process must free with `LocalFree`.
    let sid = unsafe {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        sid
    };
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_is_named_by_a_sid() {
        let sid = current_user_sid().unwrap();

        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert!(
            sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{sid}"
        );
    }
}
