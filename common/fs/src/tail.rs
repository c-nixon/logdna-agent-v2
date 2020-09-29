use crate::cache::entry::Entry;
use crate::cache::event::Event;
use crate::cache::FileSystem;
use crate::rule::Rules;
use async_trait::async_trait;
use http::types::body::LineBuilder;
use metrics::Metrics;
use source::Source;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use tokio::spawn;
use tokio::sync::mpsc::Sender;

use futures::{Stream, StreamExt};

/// Tails files on a filesystem by inheriting events from a Watcher
pub struct Tailer {
    watched_dirs: Option<Vec<PathBuf>>,
    rules: Option<Rules>,
}

impl Tailer {
    /// Creates new instance of Tailer
    pub fn new(watched_dirs: Vec<PathBuf>, rules: Rules) -> Self {
        Self {
            watched_dirs: Some(watched_dirs),
            rules: Some(rules),
        }
    }
    /// Runs the main logic of the tailer, this can only be run once so Tailer is consumed
    pub async fn process<'a>(&mut self, fs: &'a mut FileSystem<u64>, buf: &'a mut [u8]) -> Result<impl Stream<Item=Vec<LineBuilder>> + 'a, std::io::Error>{

        // let mut buf = [0u8; 4096];
        let events = {
            match fs.read_events(buf).await {
                Ok(event) => event,
                Err(e) => {
                    warn!("tailer stream raised exception: {:?}", e);
                    return Err(e);
                }
            }
        };

        Ok(events.map(move |event| {
            let mut final_lines = Vec::new();

            match event {
                Event::Initialize(mut entry_ptr) => {
                    // will initiate a file to it's current length
                    let entry = unsafe { entry_ptr.as_mut() };
                    let path = fs.resolve_direct_path(entry);

                    if let Entry::File { ref mut data, .. } = entry {
                        let mut len = path.metadata().map(|m| m.len()).unwrap_or(0);
                        if len < 8192 {
                            len = 0
                        }
                        info!("initialized {:?} with offset {}", path, len,);
                        *data = len;
                    }
                }
                Event::New(mut entry_ptr) => {
                    Metrics::fs().increment_creates();
                    // similar to initiate but sets the offset to 0
                    let entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if !paths.is_empty() {
                    if let Entry::File {
                        ref mut data,
                        file_handle,
                        ..
                    } = entry
                    {
                        info!("added {:?}", paths[0]);
                        *data = 0;
                        if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                            final_lines.append(&mut lines);
                        }
                    }
                    }


                }
                Event::Write(mut entry_ptr) => {
                    Metrics::fs().increment_writes();
                    let entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if !paths.is_empty() {

                    if let Entry::File {
                        ref mut data,
                        file_handle,
                        ..
                    } = entry
                    {
                        if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                            final_lines.append(&mut lines);
                        }
                    }

                    }
                }
                Event::Delete(mut entry_ptr) => {
                    Metrics::fs().increment_deletes();
                    let mut entry = unsafe { entry_ptr.as_mut() };
                    let paths = fs.resolve_valid_paths(entry);
                    if !paths.is_empty() {
                        if let Entry::Symlink { link, .. } = entry {
                            if let Some(real_entry) = fs.lookup(link) {
                                entry = unsafe { &mut *real_entry.as_ptr() };
                            } else {
                                error!("can't wrap up deleted symlink - pointed to file / directory doesn't exist: {:?}", paths[0]);
                            }
                        }

                        if let Entry::File {
                            ref mut data,
                            file_handle,
                            ..
                        } = entry
                        {
                            if let Some(mut lines) = Tailer::tail(file_handle, &paths, data) {
                                final_lines.append(&mut lines);
                            }
                        }

                        }
                }
            };
            futures::stream::iter(final_lines)
        }).flatten())
    }

    // tail a file for new line(s)
    fn tail(
        file_handle: &File,
        paths: &[PathBuf],
        offset: &mut u64,
    ) -> Option<Vec<Vec<LineBuilder>>> {
        // get the file len
        let len = match file_handle.metadata().map(|m| m.len()) {
            Ok(v) => v,
            Err(e) => {
                error!("unable to stat {:?}: {:?}", &paths[0], e);
                return None;
            }
        };

        // if we are at the end of the file there's no work to do
        if *offset == len {
            return None;
        }
        // open the file, create a reader
        let mut reader = BufReader::new(file_handle);
        // if the offset is greater than the file's len
        // it's very likely a truncation occurred
        if *offset > len {
            info!("{:?} was truncated from {} to {}", &paths[0], *offset, len);
            *offset = if len < 8192 { 0 } else { len };
            return None;
        }
        // seek to the offset, this creates the "tailing" effect
        if let Err(e) = reader.seek(SeekFrom::Start(*offset)) {
            error!("error seeking {:?}", e);
            return None;
        }

        let mut line_groups = Vec::new();

        loop {
            let mut raw_line = Vec::new();
            // read until a new line returning the line length
            let line_len = match reader.read_until(b'\n', &mut raw_line) {
                Ok(v) => v as u64,
                Err(e) => {
                    error!("error reading from file {:?}: {:?}", &paths[0], e);
                    break;
                }
            };
            // try to parse the raw data as utf8
            // if that fails replace invalid chars with blank chars
            // see String::from_utf8_lossy docs
            let mut line = String::from_utf8(raw_line)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
            // if the line doesn't end with a new line we might have read in the middle of a write
            // so we return in this case
            if !line.ends_with('\n') {
                Metrics::fs().increment_partial_reads();
                break;
            }
            // remove the trailing new line
            line.pop();
            // increment the offset
            *offset += line_len;
            // send the line upstream, safe to unwrap
            debug!("tailer sendings lines for {:?}", paths);
            line_groups.push(
                paths
                    .iter()
                    .map(|path| {
                        Metrics::fs().increment_lines();
                        Metrics::fs().add_bytes(line_len);
                        LineBuilder::new()
                            .line(line.clone())
                            .file(path.to_str().unwrap_or("").to_string())
                    })
                    .collect(),
            );
        }

        if line_groups.len() == 0 {
            None
        } else {
            Some(line_groups)
        }
    }
}

#[async_trait]
impl Source for Tailer {
    async fn begin_processing(&mut self, sender: Sender<Vec<LineBuilder>>) {
        let watched_dirs = match self.watched_dirs.take() {
            Some(watched_dirs) => watched_dirs,
            None => {
                error!("can't start filesystem source: missing watched directories");
                return;
            }
        };
        let rules = match self.rules.take() {
            Some(rules) => rules,
            None => {
                error!("can't start filesystem source: missing rules");
                return;
            }
        };

        spawn(async move {
            let fs = FileSystem::<u64>::new(watched_dirs, rules);
        });
    }
}
