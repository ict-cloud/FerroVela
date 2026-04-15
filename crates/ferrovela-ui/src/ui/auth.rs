/// macOS Authorization Services wrapper.
///
/// Wraps `AuthorizationCreate` + `AuthorizationCopyRights` + `AuthorizationFree`
/// from `Security.framework` to show the system "click-the-lock" admin-password
/// / Touch ID sheet, matching the System Settings pattern.
///
/// The `cfg(test)` stub short-circuits the sheet so UI tests don't require
/// user interaction.
#[cfg(not(test))]
mod native {
    use std::ffi::{c_char, c_void, CString};

    type AuthorizationRef = *mut c_void;

    #[repr(C)]
    struct AuthorizationItem {
        name: *const c_char,
        value_length: usize,
        value: *mut c_void,
        flags: u32,
    }

    #[repr(C)]
    struct AuthorizationRights {
        count: u32,
        items: *mut AuthorizationItem,
    }

    const ERR_AUTHORIZATION_SUCCESS: i32 = 0;
    const K_AUTHORIZATION_FLAG_DEFAULTS: u32 = 0;
    const K_AUTHORIZATION_FLAG_INTERACTION_ALLOWED: u32 = 1 << 0;
    const K_AUTHORIZATION_FLAG_EXTEND_RIGHTS: u32 = 1 << 1;
    const K_AUTHORIZATION_FLAG_DESTROY_RIGHTS: u32 = 1 << 3;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn AuthorizationCreate(
            rights: *const AuthorizationRights,
            environment: *const AuthorizationRights,
            flags: u32,
            authorization: *mut AuthorizationRef,
        ) -> i32;

        fn AuthorizationCopyRights(
            authorization: AuthorizationRef,
            rights: *const AuthorizationRights,
            environment: *const AuthorizationRights,
            flags: u32,
            authorized_rights: *mut *mut AuthorizationRights,
        ) -> i32;

        fn AuthorizationFree(authorization: AuthorizationRef, flags: u32) -> i32;
    }

    /// Show the macOS admin-password / Touch ID sheet for `system.preferences`.
    ///
    /// Blocks the calling thread until the user dismisses the sheet.
    /// Returns `true` on success, `false` on cancellation or error.
    /// Must be called on a background thread — not the iced runtime thread.
    pub fn request_advanced_unlock() -> bool {
        let right_name = match CString::new("system.preferences") {
            Ok(s) => s,
            Err(_) => return false,
        };

        let mut item = AuthorizationItem {
            name: right_name.as_ptr(),
            value_length: 0,
            value: std::ptr::null_mut(),
            flags: 0,
        };

        let rights = AuthorizationRights {
            count: 1,
            items: &mut item,
        };

        let mut auth_ref: AuthorizationRef = std::ptr::null_mut();

        unsafe {
            let status = AuthorizationCreate(
                std::ptr::null(),
                std::ptr::null(),
                K_AUTHORIZATION_FLAG_DEFAULTS,
                &mut auth_ref,
            );
            if status != ERR_AUTHORIZATION_SUCCESS || auth_ref.is_null() {
                return false;
            }

            let status = AuthorizationCopyRights(
                auth_ref,
                &rights,
                std::ptr::null(),
                K_AUTHORIZATION_FLAG_INTERACTION_ALLOWED | K_AUTHORIZATION_FLAG_EXTEND_RIGHTS,
                std::ptr::null_mut(),
            );

            AuthorizationFree(auth_ref, K_AUTHORIZATION_FLAG_DESTROY_RIGHTS);

            status == ERR_AUTHORIZATION_SUCCESS
        }
    }
}

#[cfg(not(test))]
pub use native::request_advanced_unlock;

/// Test stub — always succeeds so UI tests never block on the real auth sheet.
#[cfg(test)]
pub fn request_advanced_unlock() -> bool {
    true
}
