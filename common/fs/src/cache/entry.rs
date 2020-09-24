use crate::cache::Children;
use crate::rule::Rules;
use inotify::WatchDescriptor;
use std::ffi::OsString;
use std::fs::File;
use std::path::PathBuf;
use std::ptr::NonNull;

pub type EntryPtr<T> = NonNull<Entry<T>>;

#[derive(Debug)]
pub enum Entry<T> {
    File {
        name: OsString,
        parent: EntryPtr<T>,
        wd: WatchDescriptor,
        data: T,
        file_handle: File,
    },
    Dir {
        name: OsString,
        parent: Option<EntryPtr<T>>,
        children: Children<T>,
        wd: WatchDescriptor,
    },
    Symlink {
        name: OsString,
        parent: EntryPtr<T>,
        link: PathBuf,
        wd: WatchDescriptor,
        rules: Rules,
    },
}

impl<T> Entry<T> {
    pub fn name(&self) -> &OsString {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                name
            }
        }
    }

    pub fn parent(&self) -> Option<EntryPtr<T>> {
        match self {
            Entry::File { parent, .. } | Entry::Symlink { parent, .. } => Some(*parent),
            Entry::Dir { parent, .. } => *parent,
        }
    }

    pub fn set_parent(&mut self, new_parent: EntryPtr<T>) {
        match self {
            Entry::File { parent, .. } | Entry::Symlink { parent, .. } => *parent = new_parent,
            Entry::Dir { parent, .. } => *parent = Some(new_parent),
        }
    }

    pub fn set_name(&mut self, new_name: OsString) {
        match self {
            Entry::File { name, .. } | Entry::Dir { name, .. } | Entry::Symlink { name, .. } => {
                *name = new_name
            }
        }
    }

    pub fn link(&self) -> Option<&PathBuf> {
        match self {
            Entry::Symlink { link, .. } => Some(link),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Children<T>> {
        match self {
            Entry::Dir { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn watch_descriptor(&self) -> &WatchDescriptor {
        match self {
            Entry::Dir { wd, .. } | Entry::Symlink { wd, .. } | Entry::File { wd, .. } => wd,
        }
    }

    pub fn data_mut(&mut self) -> Option<&mut T> {
        match self {
            Entry::Dir { .. } | Entry::Symlink { .. } => None,
            Entry::File { data, .. } => Some(data),
        }
    }

    pub fn file_handle(&self) -> Option<&File> {
        match self {
            Entry::Dir { .. } | Entry::Symlink { .. } => None,
            Entry::File { file_handle, .. } => Some(file_handle),
        }
    }
}

unsafe impl<T> Send for Entry<T> {}
