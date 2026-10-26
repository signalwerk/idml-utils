//! The ZIP container of a package: stored and deflated entries.
//!
//! Reading goes by the central directory and checks each entry's size and
//! CRC-32. Writing gives every entry the same date (1980-01-01), so a package
//! written twice has the same bytes. Encryption, other compression methods,
//! ZIP64 and archives split over several disks are not supported; IDML
//! packages need none of them.

use crate::error::{Error, ErrorKind};
use crate::layout::Compression;

const LOCAL: u32 = 0x0403_4B50;
const CENTRAL: u32 = 0x0201_4B50;
const END: u32 = 0x0605_4B50;
/// The general purpose flag for UTF-8 names.
const UTF8: u16 = 0x0800;
/// 1980-01-01 in MS-DOS date format.
const DATE: u16 = 0x21;

/// A file of an archive.
pub(crate) struct File {
    pub name: String,
    pub compression: Compression,
    pub data: Vec<u8>,
}

fn error(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Zip, message)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The files of the archive `bytes`, in central directory order, without
/// directories. Fails when they expand to more than `max_size` bytes.
pub(crate) fn read(bytes: &[u8], max_size: u64) -> Result<Vec<File>, Error> {
    // The end of central directory record: 22 bytes and a comment of up to
    // 65535 bytes.
    let earliest = bytes.len().saturating_sub(22 + 0xFFFF);
    let end = (earliest..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(bytes, i) == Some(END))
        .ok_or_else(|| error("no end of central directory: not a ZIP archive"))?;
    let field = |at: usize| u16_at(bytes, end + at).ok_or_else(|| error("truncated end of central directory"));
    let (disk, cd_disk, count) = (field(4)?, field(6)?, field(10)?);
    let cd_offset = u32_at(bytes, end + 16).ok_or_else(|| error("truncated end of central directory"))?;
    if disk != 0 || cd_disk != 0 {
        return Err(error("archives on several disks are not supported"));
    }
    if count == 0xFFFF || cd_offset == 0xFFFF_FFFF {
        return Err(error("ZIP64 archives are not supported"));
    }

    let mut files = Vec::with_capacity(usize::from(count));
    let mut total: u64 = 0;
    let mut at = cd_offset as usize;
    for _ in 0..count {
        let truncated = || error("truncated central directory");
        if u32_at(bytes, at) != Some(CENTRAL) {
            return Err(error("bad central directory entry"));
        }
        let flags = u16_at(bytes, at + 8).ok_or_else(truncated)?;
        let method = u16_at(bytes, at + 10).ok_or_else(truncated)?;
        let crc = u32_at(bytes, at + 16).ok_or_else(truncated)?;
        let packed = u32_at(bytes, at + 20).ok_or_else(truncated)? as usize;
        let size = u32_at(bytes, at + 24).ok_or_else(truncated)?;
        let name_len = usize::from(u16_at(bytes, at + 28).ok_or_else(truncated)?);
        let extra_len = usize::from(u16_at(bytes, at + 30).ok_or_else(truncated)?);
        let comment_len = usize::from(u16_at(bytes, at + 32).ok_or_else(truncated)?);
        let local = u32_at(bytes, at + 42).ok_or_else(truncated)? as usize;
        let name = bytes.get(at + 46..at + 46 + name_len).ok_or_else(truncated)?;
        let name = String::from_utf8_lossy(name).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if flags & 1 != 0 {
            return Err(error(format!("{name}: encrypted entries are not supported")));
        }
        let compression = match method {
            0 => Compression::Stored,
            8 => Compression::Deflated,
            m => return Err(error(format!("{name}: compression method {m} is not supported"))),
        };
        if u32_at(bytes, local) != Some(LOCAL) {
            return Err(error(format!("{name}: bad local header")));
        }
        let local_name = usize::from(u16_at(bytes, local + 26).ok_or_else(|| error(format!("{name}: truncated local header")))?);
        let local_extra = usize::from(u16_at(bytes, local + 28).ok_or_else(|| error(format!("{name}: truncated local header")))?);
        let start = local + 30 + local_name + local_extra;
        let raw = bytes.get(start..start.saturating_add(packed)).ok_or_else(|| error(format!("{name}: truncated data")))?;
        total = total.saturating_add(u64::from(size));
        if total > max_size {
            return Err(Error::new(ErrorKind::Limit, format!("package expands beyond {max_size} bytes")));
        }
        let data = match compression {
            Compression::Stored => raw.to_vec(),
            Compression::Deflated => {
                miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size as usize).map_err(|e| error(format!("{name}: cannot inflate: {e}")))?
            }
        };
        if data.len() != size as usize || crc32(&data) != crc {
            return Err(error(format!("{name}: size or CRC-32 does not match")));
        }
        files.push(File { name, compression, data });
    }
    Ok(files)
}

/// The archive of `files`, in order.
pub(crate) fn write(files: &[(&str, &[u8], Compression)]) -> Result<Vec<u8>, Error> {
    let too_large = || error("package larger than 4 GiB");
    let headers: usize = files.iter().map(|(name, _, _)| 76 + 2 * name.len()).sum();
    let mut out = Vec::with_capacity(files.iter().map(|(_, data, _)| data.len()).sum::<usize>() + headers + 22);
    let mut central = Vec::new();
    for &(name, data, compression) in files {
        let offset = u32::try_from(out.len()).map_err(|_| too_large())?;
        let (method, packed) = match compression {
            Compression::Stored => (0u16, std::borrow::Cow::Borrowed(data)),
            Compression::Deflated => (8, std::borrow::Cow::Owned(miniz_oxide::deflate::compress_to_vec(data, 6))),
        };
        let crc = crc32(data);
        let size = u32::try_from(data.len()).map_err(|_| too_large())?;
        let packed_size = u32::try_from(packed.len()).map_err(|_| too_large())?;
        let name_len = u16::try_from(name.len()).map_err(|_| error(format!("{name}: name too long")))?;
        // Version needed, flags, method, time, date, CRC, sizes, name and
        // extra length: shared by the local and the central header.
        let mut common = Vec::with_capacity(26);
        common.extend_from_slice(&20u16.to_le_bytes());
        common.extend_from_slice(&UTF8.to_le_bytes());
        common.extend_from_slice(&method.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        common.extend_from_slice(&DATE.to_le_bytes());
        common.extend_from_slice(&crc.to_le_bytes());
        common.extend_from_slice(&packed_size.to_le_bytes());
        common.extend_from_slice(&size.to_le_bytes());
        common.extend_from_slice(&name_len.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&LOCAL.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(&CENTRAL.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&common);
        // Comment length, disk, internal and external attributes.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let count = u16::try_from(files.len()).map_err(|_| error("more than 65535 entries"))?;
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_large())?;
    let cd_size = u32::try_from(central.len()).map_err(|_| too_large())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    u32::try_from(out.len()).map_err(|_| too_large())?;
    Ok(out)
}

/// CRC-32 (IEEE 802.3), as ZIP uses it, eight bytes at a time.
fn crc32(data: &[u8]) -> u32 {
    const TABLES: [[u32; 256]; 8] = {
        let mut t = [[0u32; 256]; 8];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[0][i] = c;
            i += 1;
        }
        let mut i = 0;
        while i < 256 {
            let mut k = 1;
            while k < 8 {
                t[k][i] = (t[k - 1][i] >> 8) ^ t[0][(t[k - 1][i] & 0xFF) as usize];
                k += 1;
            }
            i += 1;
        }
        t
    };
    let byte = |crc: u32, b: u8| TABLES[0][((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    let mut crc = !0u32;
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let lo = crc ^ u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        crc = TABLES[7][(lo & 0xFF) as usize]
            ^ TABLES[6][((lo >> 8) & 0xFF) as usize]
            ^ TABLES[5][((lo >> 16) & 0xFF) as usize]
            ^ TABLES[4][(lo >> 24) as usize]
            ^ TABLES[3][usize::from(c[4])]
            ^ TABLES[2][usize::from(c[5])]
            ^ TABLES[1][usize::from(c[6])]
            ^ TABLES[0][usize::from(c[7])];
    }
    for &b in chunks.remainder() {
        crc = byte(crc, b);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        // Against the bytewise definition, for every remainder length.
        let bytewise = |data: &[u8]| {
            let mut crc = !0u32;
            for &b in data {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
                }
            }
            !crc
        };
        let data: Vec<u8> = (0..100u32).map(|i| (i * 37 + 11) as u8).collect();
        for len in 0..data.len() {
            assert_eq!(crc32(&data[..len]), bytewise(&data[..len]));
        }
    }

    #[test]
    fn written_archives_read_back() {
        let text = "<a>".repeat(1000);
        let files = [("mimetype", b"application/x".as_slice(), Compression::Stored), ("dir/ä.xml", text.as_bytes(), Compression::Deflated)];
        let bytes = write(&files).unwrap();
        assert_eq!(bytes.get(30..38), Some(b"mimetype".as_slice()));
        let read = read(&bytes, 1 << 20).unwrap();
        let names: Vec<(&str, Compression)> = read.iter().map(|f| (f.name.as_str(), f.compression)).collect();
        assert_eq!(names, [("mimetype", Compression::Stored), ("dir/ä.xml", Compression::Deflated)]);
        assert_eq!(read[1].data, text.as_bytes());
        assert!(matches!(super::read(&bytes, 100), Err(e) if e.kind() == ErrorKind::Limit));
    }

    #[test]
    fn damaged_archives_are_errors() {
        let bytes = write(&[("a.xml", b"<a/>".as_slice(), Compression::Deflated)]).unwrap();
        for len in 0..bytes.len() {
            assert!(read(&bytes[..len], 1 << 20).is_err());
        }
        for i in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[i] ^= 0x55;
            let _ = read(&damaged, 1 << 20);
        }
    }
}
