use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;

use futures::future::Either;
use futures::{Stream, StreamExt};
use tokio::time::Instant;

use tokio::sync::Mutex;

const INOTIFY_EVENT_GRACE_PERIOD_MS: u64 = 1000;
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

enum EventOrInterval<T> {
    Interval(Instant),
    Event(Result<inotify::Event<T>, std::io::Error>),
}

pub struct Watcher {
    inotify: Inotify,
}

impl Watcher {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            inotify: Inotify::init()?,
        })
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
        let unmatched_move_to: Arc<Mutex<Vec<(Instant, WatchEvent)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let unmatched_move_from: Arc<Mutex<Vec<(Instant, WatchEvent)>>> =
            Arc::new(Mutex::new(Vec::new()));
        // Interleave inotify events with a heartbeat every 1 second
        // heartbeat is used to ensure unpaired MOVED_TO and MOVED_FROM
        // correctly generate events.
        let events = futures::stream::select(
            self.inotify
                .event_stream(buffer)?
                .map(EventOrInterval::Event),
            tokio::time::interval(tokio::time::Duration::from_millis(
                INOTIFY_EVENT_GRACE_PERIOD_MS,
            ))
            .map(EventOrInterval::Interval),
        );
        Ok(events
            .map(move |raw_event_or_interval| {
                {
                    match raw_event_or_interval {
                        EventOrInterval::Event(raw_event) => Either::Left(futures::stream::once({
                            let unmatched_move_to = unmatched_move_to.clone();
                            let unmatched_move_from = unmatched_move_from.clone();
                            async move {
                                match raw_event {
                                    Ok(raw_event) => {
                                        Ok(if raw_event.mask.contains(EventMask::MOVED_FROM) {
                                            // Check if we have seen the corresponding MOVED_TO
                                            if let Some(idx) =
                                                unmatched_move_to.lock().await.iter().position(
                                                    |(_, event)| {
                                                        if let WatchEvent::MovedTo {
                                                            wd: _,
                                                            name: _,
                                                            cookie,
                                                        } = event
                                                        {
                                                            *cookie == raw_event.cookie
                                                        } else {
                                                            false
                                                        }
                                                    },
                                                )
                                            {
                                                // If we have seen the corresponding MOVED_TO remove it
                                                // from the unmatched vec and return a Move
                                                if let (
                                                    _,
                                                    WatchEvent::MovedTo {
                                                        wd,
                                                        name,
                                                        cookie: _,
                                                    },
                                                ) =
                                                    unmatched_move_to.lock().await.swap_remove(idx)
                                                {
                                                    Some(WatchEvent::Move {
                                                        from_wd: raw_event.wd.clone(),
                                                        from_name: raw_event
                                                            .name
                                                            .unwrap()
                                                            .to_os_string(),
                                                        to_wd: wd.clone(),
                                                        to_name: name.clone(),
                                                    })
                                                } else {
                                                    None
                                                }
                                            } else {
                                                // If we can't find the corresponding event, store this
                                                // event in the unmatched_move_from vec
                                                unmatched_move_from.lock().await.push((
                                                    Instant::now(),
                                                    WatchEvent::MovedFrom {
                                                        wd: raw_event.wd.clone(),
                                                        name: raw_event
                                                            .name
                                                            .unwrap()
                                                            .to_os_string(),
                                                        cookie: raw_event.cookie,
                                                    },
                                                ));
                                                None
                                            }
                                        } else if raw_event.mask.contains(EventMask::MOVED_TO) {
                                            if let Some(idx) =
                                                unmatched_move_from.lock().await.iter().position(
                                                    |(_, event)| {
                                                        if let WatchEvent::MovedFrom {
                                                            wd: _,
                                                            name: _,
                                                            cookie,
                                                        } = event
                                                        {
                                                            *cookie == raw_event.cookie
                                                        } else {
                                                            false
                                                        }
                                                    },
                                                )
                                            {
                                                // If we have seen the corresponding MOVED_FROM remove it
                                                // from the unmatched vec and return a Move
                                                if let (
                                                    _,
                                                    WatchEvent::MovedFrom {
                                                        wd,
                                                        name,
                                                        cookie: _,
                                                    },
                                                ) = unmatched_move_from
                                                    .lock()
                                                    .await
                                                    .swap_remove(idx)
                                                {
                                                    Some(WatchEvent::Move {
                                                        from_wd: wd.clone(),
                                                        from_name: name.clone(),
                                                        to_wd: raw_event.wd.clone(),
                                                        to_name: raw_event
                                                            .name
                                                            .unwrap()
                                                            .to_os_string(),
                                                    })
                                                } else {
                                                    None
                                                }
                                            } else {
                                                // If we can't find the corresponding event, store this
                                                // event in the unmatched_move_to vec
                                                unmatched_move_to.lock().await.push((
                                                    Instant::now(),
                                                    WatchEvent::MovedTo {
                                                        wd: raw_event.wd.clone(),
                                                        name: raw_event
                                                            .name
                                                            .unwrap()
                                                            .to_os_string(),
                                                        cookie: raw_event.cookie,
                                                    },
                                                ));
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
                                    Err(e) => Err(e),
                                }
                            }
                        })),
                        EventOrInterval::Interval(now) => {
                            Either::Right({
                                let unmatched_move_to = unmatched_move_to.clone();
                                let unmatched_move_from = unmatched_move_from.clone();

                                {
                                    let mut events = vec![];
                                    {
                                        let mut unmatched_move_to = unmatched_move_to
                                            .try_lock()
                                            .expect("Couldn't lock unmatched_move_to");
                                        while let Some(idx) =
                                            unmatched_move_to.iter().position(|(instant, _)| {
                                                now - tokio::time::Duration::from_millis(
                                                    INOTIFY_EVENT_GRACE_PERIOD_MS,
                                                ) > *instant
                                            })
                                        {
                                            events.push(Ok(Some(
                                                unmatched_move_to.swap_remove(idx).1,
                                            )));
                                        }
                                    }
                                    {
                                        let mut unmatched_move_from = unmatched_move_from
                                            .try_lock()
                                            .expect("Couldn't lock unmatched_move_to");
                                        while let Some(idx) =
                                            unmatched_move_from.iter().position(|(instant, _)| {
                                                now - tokio::time::Duration::from_millis(
                                                    INOTIFY_EVENT_GRACE_PERIOD_MS,
                                                ) > *instant
                                            })
                                        {
                                            events.push(Ok(Some(
                                                unmatched_move_from.swap_remove(idx).1,
                                            )));
                                        }
                                    }
                                    // unmatched_move_to.position
                                    futures::stream::iter(events)
                                }
                            })
                        }
                    }
                }
            })
            .flatten()
            // Unwrap the inner Option and discard unmatched events
            .filter_map(|event: Result<Option<WatchEvent>, std::io::Error>| async {
                match event {
                    Ok(None) => None,
                    event => Some(event.map(|e| e.unwrap())),
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
