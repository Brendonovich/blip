use std::ffi::CString;
use std::ptr;

use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};

/// Starts Sparkle's scheduled update checks when running from a packaged app.
pub fn start() {
    if let Err(error) = start_inner() {
        eprintln!("blip-updater: {error}");
    }
}

fn start_inner() -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("failed to locate executable: {error}"))?;
    let contents = executable
        .parent()
        .and_then(|directory| directory.parent())
        .ok_or_else(|| "executable is not inside an app bundle".to_owned())?;
    let framework = contents.join("Frameworks/Sparkle.framework/Sparkle");
    if !framework.is_file() {
        return Ok(());
    }

    let framework = CString::new(framework.to_string_lossy().as_bytes())
        .map_err(|_| "Sparkle framework path contains a null byte".to_owned())?;
    // SAFETY: Sparkle is a pinned, signed framework embedded in the app bundle. The handle and
    // updater controller intentionally live for the process lifetime.
    unsafe {
        let handle = libc::dlopen(framework.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            return Err(format!("failed to load Sparkle: {}", dlerror()));
        }

        let class = AnyClass::get(c"SPUStandardUpdaterController")
            .ok_or_else(|| "Sparkle updater class is unavailable".to_owned())?;
        let controller: *mut AnyObject = msg_send![class, alloc];
        let controller: *mut AnyObject = msg_send![controller,
            initWithStartingUpdater: true,
            updaterDelegate: ptr::null_mut::<AnyObject>(),
            userDriverDelegate: ptr::null_mut::<AnyObject>()
        ];
        if controller.is_null() {
            return Err("failed to initialize Sparkle updater".to_owned());
        }
    }

    Ok(())
}

unsafe fn dlerror() -> String {
    // SAFETY: `dlerror` returns either null or a process-owned null-terminated string.
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        return "unknown dynamic loader error".to_owned();
    }
    // SAFETY: The pointer was checked for null and follows `dlerror`'s string contract.
    unsafe { std::ffi::CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}
