//! HALCYON's filesystem: a RAM store seeded from the tar initrd.
//!
//! Files live entirely in memory and vanish at power-off. That is deliberate:
//! HALCYON contains no block-device write path at all, so booting it on a real
//! machine cannot touch what is on that machine's disks.

pub mod tar;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::SpinLock;

#[derive(Clone)]
pub struct File {
    pub data: Vec<u8>,
    /// Came from the initrd, so it cannot be overwritten in place.
    pub from_initrd: bool,
}

pub struct FileSystem {
    files: BTreeMap<String, File>,
}

impl FileSystem {
    pub const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    pub fn read(&self, path: &str) -> Option<&File> {
        self.files.get(&normalise(path))
    }

    pub fn write(&mut self, path: &str, data: Vec<u8>) -> Result<(), &'static str> {
        let path = normalise(path);
        if path == "/" {
            return Err("cannot write to the root");
        }
        self.files.insert(
            path,
            File {
                data,
                from_initrd: false,
            },
        );
        Ok(())
    }

    pub fn append(&mut self, path: &str, data: &[u8]) -> Result<(), &'static str> {
        let path = normalise(path);
        match self.files.get_mut(&path) {
            Some(file) => {
                file.data.extend_from_slice(data);
                file.from_initrd = false;
                Ok(())
            }
            None => self.write(&path, data.to_vec()),
        }
    }

    pub fn remove(&mut self, path: &str) -> Result<(), &'static str> {
        let path = normalise(path);
        match self.files.remove(&path) {
            Some(_) => Ok(()),
            None => Err("no such file"),
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.files.contains_key(&normalise(path))
    }

    /// Every path, sorted.
    pub fn list(&self) -> Vec<(String, usize, bool)> {
        self.files
            .iter()
            .map(|(path, file)| (path.clone(), file.data.len(), file.from_initrd))
            .collect()
    }

    /// Paths directly inside `directory`.
    pub fn list_dir(&self, directory: &str) -> Vec<(String, usize, bool)> {
        let mut prefix = normalise(directory);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        self.files
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, file)| (path.clone(), file.data.len(), file.from_initrd))
            .collect()
    }

    pub fn total_bytes(&self) -> usize {
        self.files.values().map(|file| file.data.len()).sum()
    }

    pub fn count(&self) -> usize {
        self.files.len()
    }
}

/// Collapse `a//b`, strip a trailing slash, and force a leading one.
pub fn normalise(path: &str) -> String {
    let mut result = String::from("/");
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }
    result
}

pub static FS: SpinLock<FileSystem> = SpinLock::new(FileSystem::new());

/// Seed the filesystem from a tar image sitting in physical memory.
pub fn mount_initrd(phys_start: u64, phys_end: u64) -> usize {
    if phys_end <= phys_start {
        return 0;
    }
    let length = (phys_end - phys_start) as usize;
    let archive = unsafe {
        core::slice::from_raw_parts(
            crate::mm::paging::phys_to_virt(phys_start) as *const u8,
            length,
        )
    };

    let mut filesystem = FS.lock();
    let mut loaded = 0;
    for entry in tar::entries(archive) {
        let Some(path) = tar::normalise(entry.name) else {
            continue;
        };
        filesystem.files.insert(
            path,
            File {
                data: entry.data.to_vec(),
                from_initrd: true,
            },
        );
        loaded += 1;
    }
    loaded
}

/// Create the directories HALCYON expects to exist, with a little content so a
/// fresh boot is not an empty machine.
pub fn seed_defaults() {
    let mut filesystem = FS.lock();
    if !filesystem.exists("/home/notes.txt") {
        let _ = filesystem.write(
            "/home/notes.txt",
            b"Scratch file. The editor saves here.\n\nNothing in HALCYON is written to disk -- this\nlives in RAM and is gone at power-off.\n"
                .to_vec(),
        );
    }
}
