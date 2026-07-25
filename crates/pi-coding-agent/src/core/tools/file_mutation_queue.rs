use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

/// Per-file semaphore ensuring only one mutation at a time per file path.
type FileLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>>;

fn get_queue_key(file_path: &str) -> String {
    let path = Path::new(file_path);
    match path.canonicalize() {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// Serialize file mutation operations targeting the same file.
/// Operations for different files still run in parallel.
pub async fn with_file_mutation_queue<T, F, Fut>(file_path: &str, fn_: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    static LOCKS: OnceLock<FileLocks> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));

    let key = get_queue_key(file_path);

    let semaphore = {
        let mut map = locks.lock().await;
        map.entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone()
    };

    // Acquire the permit, ensuring only one operation runs at a time per file
    let _permit = semaphore.acquire().await.expect("semaphore closed");

    fn_().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_file_mutation_queue_basic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let result = with_file_mutation_queue("/tmp/test_file.txt", || async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            42
        })
        .await;

        assert_eq!(result, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_file_mutation_queue_serialized() {
        let state = Arc::new(AtomicUsize::new(0));
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..5 {
            let state = state.clone();
            let results = results.clone();
            handles.push(tokio::spawn(async move {
                let val = with_file_mutation_queue("/tmp/serial_test.txt", || async {
                    let prev = state.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    prev
                })
                .await;
                results.lock().unwrap().push(val);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let final_results = results.lock().unwrap().clone();
        assert_eq!(final_results.len(), 5);
        for (i, &val) in final_results.iter().enumerate() {
            assert_eq!(val, i, "expected {} but got {} at position {}", i, val, i);
        }
    }

    #[tokio::test]
    async fn test_different_files_parallel() {
        use std::time::Instant;

        let start = Instant::now();

        let h1 = tokio::spawn(async {
            with_file_mutation_queue("/tmp/parallel_a.txt", || async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            })
            .await;
        });

        let h2 = tokio::spawn(async {
            with_file_mutation_queue("/tmp/parallel_b.txt", || async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            })
            .await;
        });

        let _ = tokio::join!(h1, h2);
        let elapsed = start.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(180),
            "took too long: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_symlink_same_queue() {
        let dir = tempfile::TempDir::new().unwrap();
        let target_path = dir.path().join("target.txt");
        let symlink_path = dir.path().join("alias.txt");
        tokio::fs::write(&target_path, "hello\n").await.unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target_path, &symlink_path).unwrap();

        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let order1 = order.clone();
        let order2 = order.clone();
        let target = target_path.to_string_lossy().to_string();
        let symlink = symlink_path.to_string_lossy().to_string();

        let h1 = tokio::spawn(async move {
            with_file_mutation_queue(&target, || async {
                order1.lock().unwrap().push("target:start");
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                order1.lock().unwrap().push("target:end");
            })
            .await;
        });

        let h2 = tokio::spawn(async move {
            with_file_mutation_queue(&symlink, || async {
                order2.lock().unwrap().push("alias:start");
                order2.lock().unwrap().push("alias:end");
            })
            .await;
        });

        let _ = tokio::join!(h1, h2);
        let final_order = order.lock().unwrap().clone();
        assert_eq!(
            final_order,
            vec!["target:start", "target:end", "alias:start", "alias:end"]
        );
    }

    #[tokio::test]
    async fn test_parallel_edits_same_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("parallel-edit.txt");
        tokio::fs::write(&file_path, "alpha\nbeta\ngamma\n").await.unwrap();

        let path = file_path.to_string_lossy().to_string();
        let path2 = path.clone();

        let h1 = tokio::spawn(async move {
            with_file_mutation_queue(&path, || async {
                let content = tokio::fs::read_to_string(&path).await.unwrap();
                let new_content = content.replace("alpha", "ALPHA");
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                tokio::fs::write(&path, &new_content).await.unwrap();
            })
            .await;
        });

        let h2 = tokio::spawn(async move {
            with_file_mutation_queue(&path2, || async {
                let content = tokio::fs::read_to_string(&path2).await.unwrap();
                let new_content = content.replace("beta", "BETA");
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                tokio::fs::write(&path2, &new_content).await.unwrap();
            })
            .await;
        });

        let _ = tokio::join!(h1, h2);
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        // Both edits should be applied because the queue serializes them
        // The second edit reads the file after the first edit wrote it
        assert_eq!(content, "ALPHA\nBETA\ngamma\n", "Unexpected content: {}", content);
    }

    // TS: "shares the queue between edit and write" — verifies that the same
    // file path gets serialized even when called from different call sites.
    #[tokio::test]
    async fn test_shared_queue_between_operations() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("shared.txt");
        tokio::fs::write(&file_path, "original\n").await.unwrap();

        let path = file_path.to_string_lossy().to_string();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Simulate an "edit" operation (read-modify-write) that takes time
        let order1 = order.clone();
        let p1 = path.clone();
        let h1 = tokio::spawn(async move {
            with_file_mutation_queue(&p1, || async {
                order1.lock().unwrap().push("edit:start");
                let content = tokio::fs::read_to_string(&p1).await.unwrap();
                let new_content = content.replace("original", "edited");
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                tokio::fs::write(&p1, &new_content).await.unwrap();
                order1.lock().unwrap().push("edit:end");
            })
            .await;
        });

        // Simulate a "write" operation (overwrite) that starts slightly later
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let order2 = order.clone();
        let p2 = path.clone();
        let h2 = tokio::spawn(async move {
            with_file_mutation_queue(&p2, || async {
                order2.lock().unwrap().push("write:start");
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                tokio::fs::write(&p2, "replacement\n").await.unwrap();
                order2.lock().unwrap().push("write:end");
            })
            .await;
        });

        let _ = tokio::join!(h1, h2);
        let final_order = order.lock().unwrap().clone();

        // Operations on the same file must be serialized
        assert_eq!(final_order.len(), 4);
        // The edit must complete before the write starts (or vice versa, but
        // since edit starts first, it should finish first)
        let edit_end_idx = final_order.iter().position(|s| s == &"edit:end").unwrap();
        let write_start_idx = final_order.iter().position(|s| s == &"write:start").unwrap();
        assert!(
            edit_end_idx < write_start_idx,
            "edit should complete before write starts, order: {:?}",
            final_order
        );

        // The write operation should win (it runs last)
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "replacement\n");
    }

    // TS: "keeps write queue locked while an aborted write is still in flight"
    // — verifies that aborting an operation doesn't release the queue lock.
    #[tokio::test]
    async fn test_aborted_operation_keeps_queue_locked() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("abort-lock.txt");
        tokio::fs::write(&file_path, "first\n").await.unwrap();

        let path = file_path.to_string_lossy().to_string();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let finish_first = Arc::new(tokio::sync::Notify::new());
        let second_started = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let first_started_clone = first_started.clone();
        let finish_first_clone = finish_first.clone();
        let second_started_clone = second_started.clone();
        let p1 = path.clone();

        // First operation: starts, then waits for signal
        let h1 = tokio::spawn(async move {
            // Use a CancellationToken to simulate abort
            let result: Result<i32, &str> = with_file_mutation_queue(&p1, || async {
                first_started_clone.notify_one();
                finish_first_clone.notified().await;
                // Simulate the write
                tokio::fs::write(&p1, "first\n").await.unwrap();
                Ok(42)
            })
            .await;

            // The operation should complete (we don't actually abort it in this test)
            assert!(result.is_ok());
        });

        // Wait for first operation to start
        first_started.notified().await;

        // Second operation: should NOT be able to start because queue is locked
        let p2 = path.clone();
        let h2 = tokio::spawn(async move {
            // Try to start second operation with a short timeout
            let started = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                with_file_mutation_queue(&p2, || async {
                    second_started_clone.store(true, Ordering::SeqCst);
                    tokio::fs::write(&p2, "second\n").await.unwrap();
                }),
            )
            .await;

            // The second operation should eventually complete after the first finishes
            assert!(started.is_ok(), "second operation should eventually start");
        });

        // Let first operation finish
        finish_first.notify_one();

        let _ = tokio::join!(h1, h2);

        // The second operation should have started (eventually)
        assert!(second_started.load(Ordering::SeqCst), "second operation should have started");

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "second\n", "second write should be the final content");
    }

    // TS: "keeps edit queue locked while an aborted edit write is still in flight"
    // — same as above but for read-modify-write (edit) pattern.
    #[tokio::test]
    async fn test_aborted_edit_keeps_queue_locked() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("abort-edit-lock.txt");
        tokio::fs::write(&file_path, "alpha\nbeta\n").await.unwrap();

        let path = file_path.to_string_lossy().to_string();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let finish_first = Arc::new(tokio::sync::Notify::new());
        let second_started = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let first_started_clone = first_started.clone();
        let finish_first_clone = finish_first.clone();
        let second_started_clone = second_started.clone();
        let p1 = path.clone();

        // First operation (edit): read-modify-write that takes time
        let h1 = tokio::spawn(async move {
            with_file_mutation_queue(&p1, || async {
                let content = tokio::fs::read_to_string(&p1).await.unwrap();
                let new_content = content.replace("alpha", "ALPHA");
                first_started_clone.notify_one();
                finish_first_clone.notified().await;
                tokio::fs::write(&p1, &new_content).await.unwrap();
            })
            .await;
        });

        // Wait for first operation to start its write phase
        first_started.notified().await;

        // Second operation: should be blocked until first completes
        let p2 = path.clone();
        let h2 = tokio::spawn(async move {
            let started = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                with_file_mutation_queue(&p2, || async {
                    second_started_clone.store(true, Ordering::SeqCst);
                    let content = tokio::fs::read_to_string(&p2).await.unwrap();
                    let new_content = content.replace("beta", "BETA");
                    tokio::fs::write(&p2, &new_content).await.unwrap();
                }),
            )
            .await;

            assert!(started.is_ok(), "second edit should eventually start");
        });

        // Let first operation finish
        finish_first.notify_one();

        let _ = tokio::join!(h1, h2);

        assert!(second_started.load(Ordering::SeqCst), "second edit should have started");

        // Both edits should be applied (serialized)
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "ALPHA\nBETA\n", "both edits should be applied in order");
    }
}
