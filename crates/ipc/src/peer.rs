//! Who is at the other end of the service's socket. The client talks only to a service run by
//! the same user, and the service's endpoint admits only that user: another local user who
//! takes the name first (Windows pipe names are shared by every session on the machine) must
//! not get the passwords that requests carry.

use std::io;

use interprocess::local_socket::tokio::{Stream, prelude::*};

fn denied(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

/// Fails unless the process at the other end of `conn` runs as this user.
#[cfg(unix)]
pub(crate) fn check(conn: &Stream) -> io::Result<()> {
    let theirs = conn
        .peer_creds()?
        .euid()
        .ok_or_else(|| denied("the socket doesn't say which user it belongs to".into()))?;
    // SAFETY: geteuid has no preconditions and can't fail.
    let mine = unsafe { libc::geteuid() };
    if theirs != mine {
        return Err(denied(format!(
            "the service socket belongs to another user (uid {theirs})"
        )));
    }
    Ok(())
}

/// Fails unless the process serving the pipe runs as this user (same token user SID).
#[cfg(windows)]
pub(crate) fn check(conn: &Stream) -> io::Result<()> {
    use windows_sys::Win32::Security::EqualSid;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let pid = conn
        .peer_creds()?
        .pid()
        .ok_or_else(|| denied("the pipe doesn't say which process serves it".into()))?;
    // SAFETY: plain call; a null handle is checked below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(denied(format!(
            "can't check who runs the process serving the pipe ({pid}): {}",
            io::Error::last_os_error()
        )));
    }
    let process = win::Handle(process);
    let theirs = win::TokenUser::of(process.0)
        .map_err(|e| denied(format!("can't check who runs process {pid}: {e}")))?;
    // SAFETY: the pseudo-handle of this process needs no closing.
    let mine = win::TokenUser::of(unsafe { GetCurrentProcess() })?;
    // SAFETY: both SIDs point into buffers that live until the end of this function.
    if unsafe { EqualSid(theirs.sid(), mine.sid()) } == 0 {
        return Err(denied(format!(
            "the service pipe is served by another user's process ({pid})"
        )));
    }
    Ok(())
}

/// A pipe security descriptor that lets only this user (its SID) in: a protected DACL with a
/// single entry, so nothing is inherited and nobody else can open the pipe.
#[cfg(windows)]
pub(crate) fn user_only()
-> io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: the pseudo-handle of this process needs no closing.
    let mine = win::TokenUser::of(unsafe { GetCurrentProcess() })?;
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: the SID lives in `mine`; on success `text` is a NUL-terminated string from the
    // local heap, freed below.
    if unsafe { ConvertSidToStringSidW(mine.sid(), &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `text` is NUL-terminated and valid until LocalFree.
    let sid = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        sid
    };
    let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    SecurityDescriptor::deserialize(&sddl)
}

#[cfg(windows)]
mod win {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser as TOKEN_USER_CLASS,
    };
    use windows_sys::Win32::System::Threading::OpenProcessToken;

    pub(super) struct Handle(pub(super) HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A process token's `TOKEN_USER`, in a buffer aligned for it.
    pub(super) struct TokenUser(Vec<u64>);

    impl TokenUser {
        pub(super) fn of(process: HANDLE) -> io::Result<Self> {
            let mut token: HANDLE = std::ptr::null_mut();
            // SAFETY: plain call; the token is closed by `Handle`.
            if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = Handle(token);
            let mut len = 0u32;
            // SAFETY: a size query with no buffer; it fails and sets `len`.
            unsafe {
                GetTokenInformation(token.0, TOKEN_USER_CLASS, std::ptr::null_mut(), 0, &mut len)
            };
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` holds at least `len` bytes.
            if unsafe {
                GetTokenInformation(
                    token.0,
                    TOKEN_USER_CLASS,
                    buf.as_mut_ptr().cast(),
                    len,
                    &mut len,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(buf))
        }

        /// Points into the buffer: valid while `self` lives.
        pub(super) fn sid(&self) -> PSID {
            // SAFETY: GetTokenInformation filled the buffer with a TOKEN_USER.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }
}
