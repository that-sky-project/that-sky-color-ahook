//! Hook 1 — certificate verification (always approve).

use crate::hooks::settings;
use crate::{log_error, log_info};
use color_hook::hook::hook;
use color_hook::memory::sigscan::{Signature, sig_scan_module_phdr};
use std::ffi::c_void;
use std::str::FromStr;
use std::sync::atomic::Ordering;

const CERT_VERIFY_PATTERN: &str = "fd 7b bd a9 f5 0b 00 f9 f4 4f 02 a9 fd 03 00 91 f3 03 00 aa 00 04 40 f9 40 01 00 b4 68 4e 40 f9";
const CERT_VERIFY_MODULE: &str = "libBootloader.so";

static mut CERT_VERIFY_BACKUP: *const c_void = std::ptr::null();

#[unsafe(no_mangle)]
#[inline(never)]
extern "C" fn fake_verify_cert(a: i32) -> i32 {
    if !settings::SKIP_CERT_VERIFY.load(Ordering::Relaxed) {
        // Pass through to the original verification.
        type VerifyFn = unsafe extern "C" fn(i32) -> i32;
        let original: VerifyFn = unsafe { std::mem::transmute(CERT_VERIFY_BACKUP) };
        unsafe { original(a) }
    } else {
        1
    }
}

pub(super) unsafe fn install() -> bool {
    let sig = match Signature::from_str(CERT_VERIFY_PATTERN) {
        Ok(s) => s,
        Err(e) => {
            log_error!("cert_verify: bad pattern — {}", e);
            return false;
        }
    };

    let target = match sig_scan_module_phdr(&sig, CERT_VERIFY_MODULE) {
        Some(addr) => {
            log_info!("cert_verify: found at 0x{:X}", addr);
            addr
        }
        None => {
            log_error!("cert_verify: pattern not found in {}", CERT_VERIFY_MODULE);
            return false;
        }
    };

    let backup = std::ptr::addr_of_mut!(CERT_VERIFY_BACKUP);
    unsafe {
        match hook(
            target as *const c_void,
            fake_verify_cert as *const c_void,
            &mut *backup,
        ) {
            Ok(()) => {
                log_info!("cert_verify: hook installed");
                true
            }
            Err(e) => {
                log_error!("cert_verify: hook failed — {}", e);
                false
            }
        }
    }
}
