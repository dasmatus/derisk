//! What is installed, as the derisk shell sees it.
//!
//! - [`desktop`]: apps and their actions from freedesktop `.desktop` files.
//! - [`apps`]: an app's name and icon, looked up by the app ID its windows
//!   carry.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod apps;
pub mod desktop;
