//! Windows: start without `vulkan-1.dll` (ADR 0004).
//!
//! `build.rs` links `vulkan-1.dll` with `/DELAYLOAD`, so the program starts without it.
//! ggml-vulkan's first call into the loader is `vkEnumerateInstanceVersion` through
//! `vkGetInstanceProcAddr`. When the DLL is missing, the hook below answers
//! `vkGetInstanceProcAddr` with a function that fails every Vulkan call; ggml turns the
//! failure into "no Vulkan backend" and whisper runs on the CPU.

use std::ffi::{c_char, c_void, CStr};

/// `DelayLoadInfo` from `delayimp.h`; only `szDll` is read.
#[repr(C)]
pub struct DelayLoadInfo {
    cb: u32,
    pidd: *const c_void,
    ppfn: *mut c_void,
    sz_dll: *const c_char,
}

type Hook = unsafe extern "C" fn(notify: u32, info: *const DelayLoadInfo) -> isize;

const DLI_FAIL_LOAD_LIB: u32 = 3;
const DLI_FAIL_GET_PROC: u32 = 4;
/// `VK_ERROR_INITIALIZATION_FAILED`.
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
}

/// Stands for every Vulkan function (`vkGetInstanceProcAddr` hands it out for any name).
extern "system" fn failing_vulkan_call() -> i32 {
    VK_ERROR_INITIALIZATION_FAILED
}

/// `vkGetInstanceProcAddr` of a Vulkan that is not there.
extern "system" fn missing_proc_addr(_instance: *mut c_void, _name: *const c_char) -> *const c_void {
    failing_vulkan_call as *const c_void
}

unsafe extern "C" fn hook(notify: u32, info: *const DelayLoadInfo) -> isize {
    let info = unsafe { &*info };
    if info.sz_dll.is_null() || !unsafe { CStr::from_ptr(info.sz_dll) }.to_bytes().eq_ignore_ascii_case(b"vulkan-1.dll") {
        return 0;
    }
    match notify {
        // Any valid module: the helper then asks it for the function and fails again below.
        DLI_FAIL_LOAD_LIB => unsafe { GetModuleHandleW(std::ptr::null()) },
        DLI_FAIL_GET_PROC => missing_proc_addr as isize,
        _ => 0,
    }
}

/// Read by `delayimp.lib`'s helper by this exact name.
#[unsafe(no_mangle)]
#[used]
pub static __pfnDliFailureHook2: Option<Hook> = Some(hook);
