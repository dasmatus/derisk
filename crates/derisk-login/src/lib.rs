//! Logging in and unlocking, without PAM or a display.
//!
//! - [`greetd`]: the login conversation `derisk greeter` holds with
//!   `derisk display-manager` (or greetd), both halves of it.
//! - [`lock`]: what the lock screen shows and when it may unlock.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod greetd;
pub mod lock;
