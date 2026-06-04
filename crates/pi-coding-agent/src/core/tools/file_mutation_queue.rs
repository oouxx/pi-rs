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
}
