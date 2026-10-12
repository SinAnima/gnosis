//! Filesystem watch: debounce change events into one incremental scan.

use crate::error::{GnosisError, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::time::Duration;

/// Block until a change under `root` has been quiet for `debounce`.
///
/// Events inside `.gnosis` and `knowledge.okf` are ignored so the knowledge
/// database and OKF export do not retrigger the scan.
pub fn wait_for_change(root: &Path, debounce: Duration) -> Result<()> {
    ChangeWatch::open(root)?.wait_quiet(debounce, None)
}

/// Owns the OS watcher for as long as a wait is in progress.
struct ChangeWatch {
    _watcher: RecommendedWatcher,
    rx: std::sync::mpsc::Receiver<std::result::Result<Event, notify::Error>>,
}

impl ChangeWatch {
    fn open(root: &Path) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |result: std::result::Result<Event, notify::Error>| {
                let _ = tx.send(result);
            },
            notify::Config::default(),
        )
        .map_err(|e| GnosisError::Pipeline(format!("watch {}: {e}", root.display())))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| GnosisError::Pipeline(format!("watch {}: {e}", root.display())))?;
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// `deadline` bounds the wait. `None` waits until a change goes quiet.
    fn wait_quiet(&self, debounce: Duration, deadline: Option<std::time::Instant>) -> Result<()> {
        loop {
            let event = self.recv(deadline)?;
            if !interesting(&event) {
                continue;
            }
            let mut quiet_until = std::time::Instant::now() + debounce;
            loop {
                let now = std::time::Instant::now();
                if now >= quiet_until {
                    return Ok(());
                }
                let mut slice = quiet_until.saturating_duration_since(now);
                if let Some(deadline) = deadline {
                    if now >= deadline {
                        return Err(watch_timeout());
                    }
                    slice = slice.min(deadline.saturating_duration_since(now));
                }
                match self.rx.recv_timeout(slice) {
                    Ok(Ok(next)) if interesting(&next) => {
                        quiet_until = std::time::Instant::now() + debounce;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        return Err(GnosisError::Pipeline(format!("watch event: {e}")));
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                            return Err(watch_timeout());
                        }
                        return Ok(());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(GnosisError::Pipeline("watch ended".into()));
                    }
                }
            }
        }
    }

    fn recv(&self, deadline: Option<std::time::Instant>) -> Result<Event> {
        let message = match deadline {
            None => self
                .rx
                .recv()
                .map_err(|e| GnosisError::Pipeline(format!("watch ended: {e}")))?,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(watch_timeout());
                }
                match self.rx.recv_timeout(remaining) {
                    Ok(message) => message,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(watch_timeout()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(GnosisError::Pipeline("watch ended".into()));
                    }
                }
            }
        };
        message.map_err(|e| GnosisError::Pipeline(format!("watch event: {e}")))
    }
}

fn watch_timeout() -> GnosisError {
    GnosisError::Pipeline("timed out waiting for a filesystem change".into())
}

fn interesting(event: &Event) -> bool {
    if event.paths.is_empty() {
        return false;
    }
    event.paths.iter().any(|path| {
        let text = path.to_string_lossy();
        !text.contains("/.gnosis") && !text.contains("\\.gnosis") && !text.contains("knowledge.okf")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use std::path::PathBuf;

    fn event(path: &str) -> Event {
        Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Any,
            )),
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        }
    }

    #[test]
    fn ignores_knowledge_db_and_okf_exports() {
        assert!(!interesting(&event("/repo/.gnosis/knowledge.redb")));
        assert!(!interesting(&event("/repo/knowledge.okf/index.md")));
        assert!(interesting(&event("/repo/src/main.rs")));
    }

    #[test]
    fn wait_returns_after_a_file_write_goes_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Open the watch before writing. A write that lands first is lost, and
        // the wait would otherwise block with no further event to wake it.
        let watch = ChangeWatch::open(&root).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            std::fs::write(root.join("a.rs"), format!("fn x() {{ /* {attempt} */ }}\n")).unwrap();
            let slice = Duration::from_millis(400)
                .min(deadline.saturating_duration_since(std::time::Instant::now()));
            assert!(
                !slice.is_zero(),
                "no filesystem event within 3s ({attempt} writes)"
            );
            match watch.wait_quiet(
                Duration::from_millis(20),
                Some(std::time::Instant::now() + slice),
            ) {
                Ok(()) => break,
                Err(_) if std::time::Instant::now() < deadline => continue,
                Err(err) => panic!("{err} after {attempt} writes"),
            }
        }
    }
}
