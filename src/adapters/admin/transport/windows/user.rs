//! Who is who, in the form a security descriptor string names them: the SID of the user the server runs as, and of the
//! user who owns a pipe that has been connected to.

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
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

    // SAFETY: the call above filled the buffer with a `TOKEN_USER` whose SID points into the same buffer, which lives
    // until the end of this function.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` is a valid SID.
    unsafe { sid_text(sid) }
}

/// The SID of the user who owns the kernel object (a pipe, here) behind `handle`.
///
/// The handle must have been opened with at least `READ_CONTROL`, which reading from it includes.
pub fn owner_sid(handle: BorrowedHandle<'_>) -> io::Result<String> {
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the handle is open for as long as it is borrowed, and the two places are valid for the answers. The owner
    // points into the descriptor, which is freed below once the SID is copied out.
    let status = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // SAFETY: on success `owner` is a valid SID inside `descriptor`, and the descriptor was allocated for this call to free.
    unsafe {
        let text = sid_text(owner);
        LocalFree(descriptor);
        text
    }
}

/// The text of a SID.
///
/// # Safety
/// `sid` must point to a valid SID.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` is valid by the caller's promise, and `text` is a place for the allocated string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `text` points to a null-terminated string that this process must free with `LocalFree`.
    unsafe {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        Ok(sid)
    }
}

/// The owner and the access list of the kernel object behind `handle` as a security descriptor string, for tests to
/// check what the system really holds against what was asked for.
#[cfg(test)]
pub fn descriptor_text(handle: BorrowedHandle<'_>) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

    let both = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: as in `owner_sid`; only the descriptor is asked for.
    let status = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_KERNEL_OBJECT,
            both,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: the descriptor is valid until freed, and `text` is a place for the allocated string.
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            both,
            &mut text,
            ptr::null_mut(),
        )
    };
    // SAFETY: allocated for this call to free.
    unsafe { LocalFree(descriptor) };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `text` is a null-terminated string to be freed with `LocalFree`.
    unsafe {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        Ok(sddl)
    }
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
