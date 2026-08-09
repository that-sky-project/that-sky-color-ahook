//! Hook implementations for ThatSkyHook.
//!
//! | Module | Hook |
//! |--------|------|
//! | [`cert_verify`] | Always-approve certificate verification |
//! | [`http_client`] | Domain replacement for HTTP requests |
//! | [`lua`] | Lua VM integration: capture state, execute scripts |
//! | [`shared`] | `NetUri` struct and domain-replacement helpers |

mod cert_verify;
mod http_client;
mod lua;
pub(crate) mod settings;
mod skylog;

// Re-export public types so callers see a flat `hooks::*` namespace.
pub use lua::{is_lua_ready, lua_exec, queue_script};

use crate::log_error;

/// Try to install every registered hook.
/// Returns the number of hooks successfully installed.
pub fn install_all() -> usize {
    let mut count = 0;

    if unsafe { cert_verify::install() } {
        count += 1;
    }

    if unsafe { http_client::install() } {
        count += 1;
    }

    if unsafe { skylog::install() } {
        count += 1;
    }

    if unsafe { lua::install() } {
        count += 1;
    } else {
        log_error!("install_all: Lua engine init failed — skipping Lua hooks");
    }

    count
}
