//! USTAR reader for the initrd.
//!
//! GRUB loads `initrd.tar` as a module; this walks it in place out of the
//! physical map. Only regular files are of interest -- directories are implied
//! by the paths.

pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

const BLOCK: usize = 512;

fn parse_octal(field: &[u8]) -> u64 {
    let mut value = 0u64;
    for &byte in field {
        match byte {
            b'0'..=b'7' => value = value * 8 + (byte - b'0') as u64,
            // Fields are NUL- or space-padded; anything else ends the number.
            _ => break,
        }
    }
    value
}

fn field_str(field: &[u8]) -> &str {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).unwrap_or("")
}

/// Iterate the regular files in a tar archive.
pub fn entries(archive: &[u8]) -> impl Iterator<Item = Entry<'_>> {
    let mut offset = 0usize;
    core::iter::from_fn(move || {
        loop {
            if offset + BLOCK > archive.len() {
                return None;
            }
            let header = &archive[offset..offset + BLOCK];

            // Two zero blocks mark the end of the archive.
            if header.iter().all(|&byte| byte == 0) {
                return None;
            }

            let magic = &header[257..262];
            if magic != b"ustar" {
                return None;
            }

            let name = field_str(&header[0..100]);
            let size = parse_octal(&header[124..136]) as usize;
            let type_flag = header[156];

            let data_start = offset + BLOCK;
            let data_end = data_start + size;
            // Payload is padded out to a block boundary.
            offset = data_start + size.div_ceil(BLOCK) * BLOCK;

            // '0' and NUL both mean a regular file.
            if (type_flag == b'0' || type_flag == 0) && data_end <= archive.len() {
                return Some(Entry {
                    name,
                    data: &archive[data_start..data_end],
                });
            }
            // Directory, link, or truncated: skip and keep going.
        }
    })
}

/// Turn a tar member name into an absolute HALCYON path.
///
/// `tar -C dir -cf out .` produces names like `./motd.txt`.
pub fn normalise(name: &str) -> Option<alloc::string::String> {
    use alloc::string::{String, ToString};

    let trimmed = name.trim_start_matches("./").trim_start_matches('/');
    if trimmed.is_empty() || trimmed.ends_with('/') {
        return None;
    }
    let mut path = String::from("/");
    path.push_str(&trimmed.to_string());
    Some(path)
}
