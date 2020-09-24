use inotify::{EventMask, EventStream, Inotify, WatchDescriptor, WatchMask};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::collections::VecDeque;
use std::task::Poll;
use futures::poll;
use futures_util::StreamExt;
use futures_core::stream::Stream;
use owning_ref::BoxRefMut;

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

struct EventStreamWrapper<'a> {
    event_stream: EventStream<&'a mut [u8]>
}

impl <'a> EventStreamWrapper<'a> {
    pub fn new(inotify: &Inotify, &mut BoxRefMut<[u8]>) -> Self {
        Self {
            event_stream: inotify.event_stream(&mut *BoxRefMut).unwrap(),
        }
    }

    pub fn as_ref(&self) -> &EventStream<&'a mut [u8]> {
        &self.event_stream
    }

    pub fn as_mut(&mut self) -> &mut EventStream<&'a mut [u8]> {
        &mut self.event_stream
    }
}

pub struct Watcher<'a> {
    inotify: Inotify,
    buffer: BoxRefMut<[u8]>,
    event_stream_wrapper: EventStreamWrapper<'a>,
    events: VecDeque<WatchEvent>,
}

impl<'a> Watcher<'a> {
    pub fn new() -> io::Result<Self> {
        let mut inotify = Inotify::init()?;
        let mut buffer = BoxRefMut::new(Box::new([0u8; 4096]) as Box<[u8]>);
        let event_stream_wrapper = EventStreamWrapper::new(&mut buffer);
        Ok(Self {
            inotify,
            buffer,
            event_stream_wrapper,
            events: VecDeque::new(),
        })
    }

    pub fn watch<P: AsRef<Path>>(&mut self, path: P) -> io::Result<WatchDescriptor> {
        self.inotify
            .add_watch(path.as_ref(), watch_mask(path.as_ref()))
    }

    pub fn unwatch(&mut self, wd: WatchDescriptor) -> io::Result<()> {
        self.inotify.rm_watch(wd)
    }

    pub async fn read_events(&mut self) -> Option<io::Result<Vec<WatchEvent>>> {
        let event_stream = self.event_stream_wrapper.as_mut();
        let (stream_min_size_hint, _) = event_stream.size_hint();
        self.events.reserve(stream_min_size_hint);
        let mut raw_events = Vec::with_capacity(stream_min_size_hint);

        loop {
            let poll_value = poll!(event_stream.next());

            match poll_value {
                Poll::Ready(Some(Ok(stream_item))) => raw_events.push(stream_item),
                Poll::Ready(Some(Err(e))) => return Some(Err(e)),
                Poll::Ready(None) => return None,
                Poll::Pending => break,
            }
        }

        Some(Ok(vec![WatchEvent::Overflow]))

        /*
        for raw_event in self.inotify.read_events_blocking(buffer)? {
            if raw_event.mask.contains(EventMask::MOVED_FROM) {
                let mut found_match = false;
                for event in events.iter_mut() {
                    match event {
                        WatchEvent::MovedTo { wd, name, cookie } => {
                            if *cookie == raw_event.cookie {
                                *event = WatchEvent::Move {
                                    from_wd: raw_event.wd.clone(),
                                    from_name: raw_event.name.unwrap().to_os_string(),
                                    to_wd: wd.clone(),
                                    to_name: name.clone(),
                                };
                                found_match = true;
                                break;
                            }
                        }
                        _ => continue,
                    };
                }

                if !found_match {
                    events.push(WatchEvent::MovedFrom {
                        wd: raw_event.wd.clone(),
                        name: raw_event.name.unwrap().to_os_string(),
                        cookie: raw_event.cookie,
                    });
                }
            } else if raw_event.mask.contains(EventMask::MOVED_TO) {
                let mut found_match = false;
                for event in events.iter_mut() {
                    match event {
                        WatchEvent::MovedFrom { wd, name, cookie } => {
                            if *cookie == raw_event.cookie {
                                *event = WatchEvent::Move {
                                    from_wd: wd.clone(),
                                    from_name: name.clone(),
                                    to_wd: raw_event.wd.clone(),
                                    to_name: raw_event.name.unwrap().to_os_string(),
                                };
                                found_match = true;
                                break;
                            }
                        }
                        _ => continue,
                    };
                }

                if !found_match {
                    events.push(WatchEvent::MovedTo {
                        wd: raw_event.wd.clone(),
                        name: raw_event.name.unwrap().to_os_string(),
                        cookie: raw_event.cookie,
                    });
                }
            } else if raw_event.mask.contains(EventMask::CREATE) {
                events.push(WatchEvent::Create {
                    wd: raw_event.wd.clone(),
                    name: raw_event.name.unwrap().to_os_string(),
                });
            } else if raw_event.mask.contains(EventMask::DELETE) {
                events.push(WatchEvent::Delete {
                    wd: raw_event.wd.clone(),
                    name: raw_event.name.unwrap().to_os_string(),
                });
            } else if raw_event.mask.contains(EventMask::MODIFY) {
                events.push(WatchEvent::Modify {
                    wd: raw_event.wd.clone(),
                });
            } else if raw_event.mask.contains(EventMask::Q_OVERFLOW) {
                events.push(WatchEvent::Overflow);
            }
        }
        Ok(events)
        */
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
