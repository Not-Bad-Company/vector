//! A small pool of worker threads for blocking work the window waits on. Previews get their
//! own pool so they never queue behind hundreds of thumbnails or a folder walk.

use std::sync::{Arc, Mutex, mpsc};

type Job = Box<dyn FnOnce() + Send>;

pub struct Pool {
    jobs: Mutex<mpsc::Sender<Job>>,
}

impl Pool {
    /// `threads` workers named `name`.
    pub fn new(name: &str, threads: usize) -> Pool {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads.max(1) {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(r) => r.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(f) => f(),
                            Err(_) => return,
                        }
                    }
                });
        }
        Pool {
            jobs: Mutex::new(tx),
        }
    }

    /// Runs `f` on a worker; await the result. `None` if the job panicked or the pool is gone.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = async_channel::bounded(1);
        let job: Job = Box::new(move || {
            if let Ok(v) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                let _ = tx.send_blocking(v);
            }
        });
        self.jobs.lock().ok()?.send(job).ok()?;
        rx.recv().await.ok()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runs_jobs_and_survives_panics() {
        let pool = super::Pool::new("test", 2);
        let r = futures_block_on(pool.run(|| 6 * 7));
        assert_eq!(r, Some(42));
        let p: Option<()> = futures_block_on(pool.run(|| panic!("boom")));
        assert_eq!(p, None);
        assert_eq!(
            futures_block_on(pool.run(|| "still here")),
            Some("still here")
        );
    }

    fn futures_block_on<T>(f: impl std::future::Future<Output = T>) -> T {
        glib::MainContext::new().block_on(f)
    }
}
