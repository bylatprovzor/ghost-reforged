use super::{Book, Record};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::Duration,
};

#[derive(Default)]
struct Pending {
    updates: BTreeMap<String, Record>,
    closing: bool,
}
type Shared = Arc<(Mutex<Pending>, Condvar)>;

pub(super) struct Writer {
    shared: Shared,
    warnings: mpsc::Receiver<String>,
    done: mpsc::Receiver<()>,
}

impl Writer {
    pub fn open(path: &Path) -> io::Result<(Self, Book)> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("No history directory"))?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        FileExt::try_lock_exclusive(&lock)?;
        let book: Book = match fs::metadata(path) {
            Ok(meta) if meta.len() <= 64 * 1024 * 1024 => serde_json::from_slice(&fs::read(path)?)?,
            Ok(_) => return Err(io::Error::other("History is too large")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Book::default(),
            Err(e) => return Err(e),
        };
        if book.version != 1
            || book.players.iter().any(|(k, v)| {
                k != &v.name.to_lowercase() || v.name.len() > 160 || v.first_seen > v.last_seen
            })
        {
            return Err(io::Error::other("Unsupported history"));
        }
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let writer_shared = shared.clone();
        let writer_book = book.clone();
        let path = path.to_owned();
        let (tx, warnings) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        thread::Builder::new()
            .name("ghost-stats".into())
            .spawn(move || {
                persist_loop(path, lock, writer_book, writer_shared, tx);
                let _ = finished.send(());
            })?;
        Ok((
            Self {
                shared,
                warnings,
                done,
            },
            book,
        ))
    }
    pub fn update(&self, name: String, record: Record) {
        if let Ok(mut pending) = self.shared.0.lock() {
            pending.updates.insert(name, record);
            self.shared.1.notify_one();
        }
    }
    pub fn warning(&self) -> Option<String> {
        self.warnings.try_recv().ok()
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.shared.0.lock() {
            pending.closing = true;
            self.shared.1.notify_one();
        }
        // Normal shutdown flushes. A stuck disk must not hold the hosting worker forever.
        // The detached writer retains its file lock until it finishes.
        let _ = self.done.recv_timeout(Duration::from_millis(500));
    }
}

fn persist_loop(
    path: PathBuf,
    _lock: File,
    mut book: Book,
    shared: Shared,
    warnings: mpsc::Sender<String>,
) {
    let mut dirty = false;
    let mut failed = false;
    loop {
        let Ok(pending) = shared.0.lock() else {
            return;
        };
        let Ok((mut pending, _)) =
            shared
                .1
                .wait_timeout_while(pending, Duration::from_secs(2), |p| {
                    !p.closing && p.updates.is_empty()
                })
        else {
            return;
        };
        let closing = pending.closing;
        let updates = std::mem::take(&mut pending.updates);
        drop(pending); // No serialization or filesystem operation under this lock.
        dirty |= !updates.is_empty();
        book.players.extend(updates);
        if dirty {
            match save(&path, &book) {
                Ok(()) => {
                    dirty = false;
                    if failed {
                        let _ = warnings.send("Statistics storage recovered.".into());
                    }
                    failed = false;
                }
                Err(_) => {
                    if !failed {
                        let _ = warnings.send(
                            "Statistics could not be saved; retrying in the background.".into(),
                        );
                    }
                    failed = true;
                }
            }
        }
        if closing {
            return;
        }
    }
}

fn save(path: &Path, book: &Book) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    serde_json::to_writer(&mut file, book)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
