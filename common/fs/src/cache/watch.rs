use futures::{Stream, StreamExt};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::ffi::OsString;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum WatchEvent {
    Create {
        wd: WatchDescriptor,
        name: OsString,
    },
    Modify {
        wd: WatchDescriptor,
    },
    Delete {
        wd: WatchDescriptor,
        name: OsString,
    },
    Move {
        from_wd: WatchDescriptor,
        from_name: OsString,
        to_wd: WatchDescriptor,
        to_name: OsString,
    },
    MovedFrom {
        wd: WatchDescriptor,
        name: OsString,
        cookie: u32,
    },
    MovedTo {
        wd: WatchDescriptor,
        name: OsString,
        cookie: u32,
    },
    Overflow,
}

pub struct Watcher {
    inotify: Inotify,
}

impl Watcher {
    pub fn new() -> io::Result<Self> {
        let mut inotify = Inotify::init()?;
        Ok(Self { inotify })
    }

    pub fn watch<P: AsRef<Path>>(&mut self, path: P) -> io::Result<WatchDescriptor> {
        self.inotify
            .add_watch(path.as_ref(), watch_mask(path.as_ref()))
    }

    pub fn unwatch(&mut self, wd: WatchDescriptor) -> io::Result<()> {
        self.inotify.rm_watch(wd)
    }

    pub fn read_events<'a>(
        &mut self,
        buffer: &'a mut [u8],
    ) -> std::io::Result<impl Stream<Item = Result<WatchEvent, std::io::Error>> + 'a> {
        let mut unmatched_move_to = Vec::new();
        let mut unmatched_move_from = Vec::new();
        Ok(self
            .inotify
            .event_stream(buffer)?
            .map(move |raw_event| {
                match raw_event {
                    Ok(raw_event) => {
                        Ok(if raw_event.mask.contains(EventMask::MOVED_FROM) {
                            // Check if we have seen the corresponding MOVED_TO
                            if let Some(idx) = unmatched_move_to.iter().position(|event| {
                                if let WatchEvent::MovedTo { wd, name, cookie } = event {
                                    *cookie == raw_event.cookie
                                } else {
                                    false
                                }
                            }) {
                                // If we have seen the corresponding MOVED_TO remove it
                                // from the unmatched vec and return a Move
                                if let WatchEvent::MovedTo { wd, name, cookie } =
                                    unmatched_move_to.swap_remove(idx)
                                {
                                    Some(WatchEvent::Move {
                                        from_wd: raw_event.wd.clone(),
                                        from_name: raw_event.name.unwrap().to_os_string(),
                                        to_wd: wd.clone(),
                                        to_name: name.clone(),
                                    })
                                } else {
                                    None
                                }
                            } else {
                                // If we can't find the corresponding event, store this
                                // event in the unmatched_move_from vec
                                unmatched_move_from.push(WatchEvent::MovedFrom {
                                    wd: raw_event.wd.clone(),
                                    name: raw_event.name.unwrap().to_os_string(),
                                    cookie: raw_event.cookie,
                                });
                                None
                            }
                        } else if raw_event.mask.contains(EventMask::MOVED_TO) {
                            if let Some(idx) = unmatched_move_from.iter().position(|event| {
                                if let WatchEvent::MovedFrom { wd, name, cookie } = event {
                                    *cookie == raw_event.cookie
                                } else {
                                    false
                                }
                            }) {
                                // If we have seen the corresponding MOVED_FROM remove it
                                // from the unmatched vec and return a Move
                                if let WatchEvent::MovedFrom { wd, name, cookie } =
                                    unmatched_move_from.swap_remove(idx)
                                {
                                    Some(WatchEvent::Move {
                                        from_wd: wd.clone(),
                                        from_name: name.clone(),
                                        to_wd: raw_event.wd.clone(),
                                        to_name: raw_event.name.unwrap().to_os_string(),
                                    })
                                } else {
                                    None
                                }
                            } else {
                                // If we can't find the corresponding event, store this
                                // event in the unmatched_move_to vec
                                unmatched_move_to.push(WatchEvent::MovedTo {
                                    wd: raw_event.wd.clone(),
                                    name: raw_event.name.unwrap().to_os_string(),
                                    cookie: raw_event.cookie,
                                });
                                None
                            }
                        } else if raw_event.mask.contains(EventMask::CREATE) {
                            Some(WatchEvent::Create {
                                wd: raw_event.wd.clone(),
                                name: raw_event.name.unwrap().to_os_string(),
                            })
                        } else if raw_event.mask.contains(EventMask::DELETE) {
                            Some(WatchEvent::Delete {
                                wd: raw_event.wd.clone(),
                                name: raw_event.name.unwrap().to_os_string(),
                            })
                        } else if raw_event.mask.contains(EventMask::MODIFY) {
                            Some(WatchEvent::Modify {
                                wd: raw_event.wd.clone(),
                            })
                        } else if raw_event.mask.contains(EventMask::Q_OVERFLOW) {
                            Some(WatchEvent::Overflow)
                        } else {
                            None
                        })
                    }
                    Err(e) => Err(Some(e)),
                }
            })
            // Unwrap the inner Option and discard unmatched events
            .filter_map(|event| async move {
                match event {
                    Ok(None) => None,
                    event => Some(event.map(|e| e.unwrap()).map_err(|e| e.unwrap())),
                }
            }))
    }
}

// returns the watch mask depending on if a path is a file or dir
fn watch_mask<P: AsRef<Path>>(path: P) -> WatchMask {
    if path.as_ref().is_file() {
        WatchMask::MODIFY | WatchMask::DONT_FOLLOW
    } else {
        WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::DONT_FOLLOW
            | WatchMask::MOVED_TO
            | WatchMask::MOVED_FROM
    }
}
