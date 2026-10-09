//! Running analysis on a thread with a large stack
//!
//! sqlparser builds, measures ([`Spanned`](sqlparser::ast::Spanned)) and drops
//! expression trees recursively, and a chain of left-associative binary operators
//! (`id = 1 OR id = 2 OR ...`, generated `IN`-like filters) nests as deep as it is
//! long. On the default stack of a spawned thread (2 MiB) a few thousand terms
//! overflow it, which aborts the whole process. Binaries that analyze SQL run the
//! analysis with [`with_analysis_stack`] or spawn their worker threads with
//! [`ANALYSIS_STACK_SIZE`].

/// Stack size for threads that parse and analyze SQL. Only the pages a thread
/// touches are committed, so this costs nothing for ordinary queries; it lets
/// expressions of hundreds of thousands of terms be analyzed.
pub const ANALYSIS_STACK_SIZE: usize = 256 * 1024 * 1024;

/// Run `f` on a thread with an [`ANALYSIS_STACK_SIZE`] stack and return its result
/// (a panic in `f` is propagated). If the thread can't be spawned (or on
/// WebAssembly, which has no threads), `f` runs on the current thread.
///
/// # Example
///
/// ```
/// use sqlsift_core::stack::with_analysis_stack;
///
/// assert_eq!(with_analysis_stack(|| 1 + 1), 2);
/// ```
pub fn with_analysis_stack<R: Send, F: FnOnce() -> R + Send>(f: F) -> R {
    #[cfg(not(target_family = "wasm"))]
    {
        let mut f = Some(f);
        let slot = &mut f;
        let result = std::thread::scope(|scope| {
            let handle = std::thread::Builder::new()
                .stack_size(ANALYSIS_STACK_SIZE)
                .spawn_scoped(scope, move || slot.take().map(|f| f()))
                .ok()?;
            match handle.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        });
        match (result, f) {
            (Some(result), _) => result,
            // The thread couldn't be spawned
            (None, Some(f)) => f(),
            (None, None) => unreachable!("the analysis thread ran but returned nothing"),
        }
    }
    #[cfg(target_family = "wasm")]
    {
        f()
    }
}
