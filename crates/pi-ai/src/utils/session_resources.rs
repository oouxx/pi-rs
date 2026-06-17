//! Session-scoped resource registration and cleanup.
//!
//! Ported from `packages/ai/src/session-resources.ts`.

use std::sync::Mutex;

type CleanupFn = Box<dyn Fn(Option<&str>) + Send>;

static CLEANUPS: std::sync::LazyLock<Mutex<Vec<(usize, CleanupFn)>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

/// Register a cleanup callback that will be invoked when session resources
/// are cleaned up. Returns an unregister function.
///
/// # Example
/// ```
/// use pi_ai::utils::session_resources::register_session_resource_cleanup;
///
/// let unregister = register_session_resource_cleanup(Box::new(|session_id| {
///     if let Some(id) = session_id {
///         println!("Cleaning up session: {}", id);
///     }
/// }));
/// // Later: unregister(); // to remove without waiting for cleanup
/// ```
pub fn register_session_resource_cleanup(cleanup: CleanupFn) -> Box<dyn Fn() + Send> {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    {
        let mut cleanups = CLEANUPS.lock().unwrap();
        cleanups.push((id, cleanup));
    }
    let unregister_id = id;
    Box::new(move || {
        let mut cleanups = CLEANUPS.lock().unwrap();
        cleanups.retain(|(cid, _)| *cid != unregister_id);
    })
}

/// Invoke all registered cleanup callbacks. If any throw/return an error,
/// an error string with all collected errors is returned.
pub fn cleanup_session_resources(session_id: Option<&str>) -> Result<(), String> {
    let cleanups = {
        let mut c = CLEANUPS.lock().unwrap();
        std::mem::take(&mut *c)
    };

    let mut errors: Vec<String> = Vec::new();
    for (_, cleanup) in cleanups {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cleanup(session_id))) {
            Ok(()) => {}
            Err(e) => {
                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown error".to_string()
                };
                errors.push(msg);
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to cleanup session resources: {}",
            errors.join("; ")
        ))
    }
}

/// Clear all registered cleanup callbacks without invoking them.
pub fn clear_cleanups() {
    let mut cleanups = CLEANUPS.lock().unwrap();
    cleanups.clear();
}

