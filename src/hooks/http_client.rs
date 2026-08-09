//! Hook 2 — HttpClient::Request (domain replacement).

use crate::hooks::settings;
use crate::{log_error, log_info};
use color_hook::hook::hook;
use color_hook::memory::sigscan::{Signature, sig_scan_module_phdr};
use std::ffi::{CStr, c_void};
use std::str::FromStr;
use std::sync::atomic::Ordering;

const HTTP_CLIENT_PATTERN: &str = "E8 0F 19 FC FD 7B 01 A9 FC 6F 02 A9 FA 67 03 A9 F8 5F 04 A9 F6 57 05 A9 F4 4F 06 A9 FD 43 00 91 E9 C3 25 D1 3F E9 7B 92 29 00 40 B9 08 8D 8E 52";
const HTTP_CLIENT_MODULE: &str = "libBootloader.so";

static mut HTTP_CLIENT_BACKUP: *const c_void = std::ptr::null();

type HttpClientRequestFn = unsafe extern "C" fn(
    this: *mut c_void,
    net_uri: *mut NetUri,
    a3: *mut c_void,
    a4: *mut c_void,
) -> u64;

#[repr(C)]
pub struct NetUri {
    pub protocol: [u8; 8],
    pub domain: [u8; 128],
    pub port: [u8; 6],
    pub path: [u8; 256],
}

/// Copy `src` into `dst`, null-terminating within bounds.
pub(crate) fn strcpy_fixed<const N: usize>(dst: &mut [u8; N], src: &str) {
    let src_bytes = src.as_bytes();
    let copy_len = src_bytes.len().min(N.saturating_sub(1));
    dst[..copy_len].copy_from_slice(&src_bytes[..copy_len]);
    dst[copy_len] = 0;
}

/// Apply the active settings (force HTTP + rewrite rules) to a [`NetUri`].
pub(crate) unsafe fn replace_domain(net_uri: *mut NetUri) {
    use crate::log_info;
    use std::ffi::CStr;

    unsafe {
        if settings::FORCE_HTTP.load(Ordering::Relaxed) {
            strcpy_fixed(&mut (*net_uri).protocol, "http");
        }

        if !settings::REWRITE_DOMAIN.load(Ordering::Relaxed) {
            return;
        }

        let domain_cstr = CStr::from_ptr((*net_uri).domain.as_ptr() as *const libc::c_char);
        let port_cstr = CStr::from_ptr((*net_uri).port.as_ptr() as *const libc::c_char);
        let Ok(domain_str) = domain_cstr.to_str() else {
            return;
        };
        let port_str = port_cstr.to_str().unwrap_or("");

        for rule in settings::rules() {
            let origin_port_matches = rule.origin_port.as_deref().map_or(true, |p| p == port_str);
            if domain_str == rule.origin && origin_port_matches {
                strcpy_fixed(&mut (*net_uri).domain, &rule.target);
                if let Some(target_port) = &rule.target_port {
                    strcpy_fixed(&mut (*net_uri).port, target_port);
                }
                log_info!(
                    "[HTTP_CLIENT] rewritten {}:{} -> {}:{}",
                    rule.origin,
                    rule.origin_port.as_deref().unwrap_or("*"),
                    rule.target,
                    rule.target_port.as_deref().unwrap_or(port_str)
                );
                break;
            }
        }
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
extern "C" fn hook_http_client_request(
    this: *mut c_void,
    net_uri: *mut NetUri,
    a3: *mut c_void,
    a4: *mut c_void,
) -> u64 {
    unsafe { replace_domain(net_uri) };

    unsafe {
        let protocol = CStr::from_ptr((*net_uri).protocol.as_ptr() as *const libc::c_char);
        let domain = CStr::from_ptr((*net_uri).domain.as_ptr() as *const libc::c_char);
        let port = CStr::from_ptr((*net_uri).port.as_ptr() as *const libc::c_char);
        let path = CStr::from_ptr((*net_uri).path.as_ptr() as *const libc::c_char);

        log_info!(
            "[HTTP_CLIENT] URL={}://{}:{}{}",
            protocol.to_str().unwrap_or("?"),
            domain.to_str().unwrap_or("?"),
            port.to_str().unwrap_or("?"),
            path.to_str().unwrap_or("?")
        );
    }

    let original: HttpClientRequestFn = unsafe { std::mem::transmute(HTTP_CLIENT_BACKUP) };
    unsafe { original(this, net_uri, a3, a4) }
}

pub(super) unsafe fn install() -> bool {
    let sig = match Signature::from_str(HTTP_CLIENT_PATTERN) {
        Ok(s) => s,
        Err(e) => {
            log_error!("http_client: bad pattern — {}", e);
            return false;
        }
    };

    let target = match sig_scan_module_phdr(&sig, HTTP_CLIENT_MODULE) {
        Some(addr) => {
            log_info!("http_client: found at 0x{:X}", addr);
            addr
        }
        None => {
            log_error!("http_client: pattern not found in {}", HTTP_CLIENT_MODULE);
            return false;
        }
    };

    let backup = std::ptr::addr_of_mut!(HTTP_CLIENT_BACKUP);
    unsafe {
        match hook(
            target as *const c_void,
            hook_http_client_request as *const c_void,
            &mut *backup,
        ) {
            Ok(()) => {
                log_info!("http_client: hook installed");
                true
            }
            Err(e) => {
                log_error!("http_client: hook failed — {}", e);
                false
            }
        }
    }
}
