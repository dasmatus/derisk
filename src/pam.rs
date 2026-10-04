//! Password checks for the lock screen, through PAM.
//!
//! This is the one place derisk calls C directly, which is why it lives in
//! the binary and not in the library (`#![forbid(unsafe_code)]`). It asks the
//! `derisk` PAM service (`data/pam.d/derisk`) to authenticate the session's
//! user, the same way swaylock and GNOME's lock screen do, so whatever the
//! system's PAM stack does at login -- `pam_unix`, `pam_systemd_home`,
//! fingerprint, faillock -- applies here too.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
};

/// The PAM service the lock screen authenticates against.
pub const SERVICE: &str = "derisk";

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: ConvFn,
    appdata_ptr: *mut c_void,
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut c_void,
    ) -> c_int;
    fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut c_void, pam_status: c_int) -> c_int;
}

/// What the conversation answers with.
struct Answers {
    user: CString,
    password: CString,
}

/// Answers PAM's prompts: the password to hidden ones, the user name to
/// visible ones, nothing to informational messages. PAM frees the responses
/// with `free`, so they are allocated with `calloc` and `strdup`.
unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(num_msg) else {
        return PAM_CONV_ERR;
    };
    if count == 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: appdata_ptr is the `Answers` passed to pam_start, alive for the
    // whole pam_authenticate call.
    let answers = unsafe { &*appdata_ptr.cast::<Answers>() };
    // SAFETY: calloc returns zeroed memory or null, checked below.
    let replies = unsafe { libc::calloc(count, size_of::<PamResponse>()) }.cast::<PamResponse>();
    if replies.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..count {
        // SAFETY: Linux-PAM passes an array of `count` message pointers.
        let message = unsafe { &**msg.add(i) };
        let answer = match message.msg_style {
            PAM_PROMPT_ECHO_OFF => Some(&answers.password),
            PAM_PROMPT_ECHO_ON => Some(&answers.user),
            _ => None,
        };
        if let Some(answer) = answer {
            // SAFETY: `replies` holds `count` zeroed responses; strdup copies
            // a NUL-terminated string.
            unsafe { (*replies.add(i)).resp = libc::strdup(answer.as_ptr()) };
        }
    }
    // SAFETY: resp is PAM's out-pointer for the response array.
    unsafe { *resp = replies };
    PAM_SUCCESS
}

/// Checks `password` for `user`. Returns whether PAM accepted it and the
/// account may log in now (not expired or locked).
pub fn authenticate(user: &str, mut password: String) -> bool {
    let ok = check(user, &password);
    wipe(&mut password);
    ok
}

fn check(user: &str, password: &str) -> bool {
    let (Ok(service), Ok(user), Ok(password)) = (
        CString::new(SERVICE),
        CString::new(user),
        CString::new(password),
    ) else {
        return false;
    };
    let mut answers = Answers { user, password };
    let conversation = PamConv {
        conv: converse,
        appdata_ptr: (&raw mut answers).cast(),
    };
    let mut handle = ptr::null_mut();
    // SAFETY: every pointer is valid for the duration of the calls, and
    // `handle` is ended exactly once whenever pam_start set it.
    let ok = unsafe {
        let started = pam_start(
            service.as_ptr(),
            answers.user.as_ptr(),
            &conversation,
            &mut handle,
        );
        if started != PAM_SUCCESS || handle.is_null() {
            false
        } else {
            let mut status = pam_authenticate(handle, 0);
            if status == PAM_SUCCESS {
                status = pam_acct_mgmt(handle, 0);
            }
            pam_end(handle, status);
            status == PAM_SUCCESS
        }
    };
    let mut bytes = answers.password.into_bytes();
    bytes.fill(0);
    ok
}

/// Overwrites the password's bytes before the allocation is freed.
fn wipe(password: &mut str) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { password.as_bytes_mut() }.fill(0);
}

/// The user the session runs as, for PAM.
pub fn current_user() -> Option<String> {
    // SAFETY: getuid cannot fail. getpwuid_r writes into `entry` and `buf`,
    // and `name` is read only while `buf` is alive.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as c_char; 4096];
        let mut result = ptr::null_mut();
        let status = libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if status != 0 || result.is_null() || entry.pw_name.is_null() {
            return None;
        }
        CStr::from_ptr(entry.pw_name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}
