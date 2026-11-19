//! The ZIP container of a package: stored and deflated entries.
//!
//! Reading goes by the central directory and checks each entry's size and
//! CRC-32. Each local header must agree with its central entry about the name
//! and method, and about CRC-32 and sizes unless a data descriptor carries them;
//! entries must not share or overlap bytes. Archives that different readers
//! could interpret differently are rejected rather than guessed at.
//! Writing gives every entry the same date (1980-01-01), so a package
//! written twice has the same bytes. Encryption, other compression methods,
//! ZIP64 and archives split over several disks are not supported; IDML
//! packages need none of them.

use crate::ReadOptions;
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
/// directories. Enforces entry count and expanded-size budgets before allocation.
pub(crate) fn read(bytes: &[u8], options: &ReadOptions) -> Result<Vec<File>, Error> {
    // The end of central directory record: 22 bytes and a comment of up to
    // 65535 bytes, which must end exactly at the end of the input. A signature
    // whose comment would be truncated or followed by other bytes is not it.
    let earliest = bytes.len().saturating_sub(22 + 0xFFFF);
    let candidates = || (earliest..=bytes.len().saturating_sub(22)).rev().filter(|&i| u32_at(bytes, i) == Some(END));
    let end = candidates().find(|&i| u16_at(bytes, i + 20).is_some_and(|c| i + 22 + usize::from(c) == bytes.len())).ok_or_else(|| {
        if candidates().next().is_some() {
            error("end of central directory comment does not end the archive: truncated or trailing data")
        } else {
            error("no end of central directory: not a ZIP archive")
        }
    })?;
    let field = |at: usize| u16_at(bytes, end + at).ok_or_else(|| error("truncated end of central directory"));
    let (disk, cd_disk, on_disk, count) = (field(4)?, field(6)?, field(8)?, field(10)?);
    let cd_size = u32_at(bytes, end + 12).ok_or_else(|| error("truncated end of central directory"))?;
    let cd_offset = u32_at(bytes, end + 16).ok_or_else(|| error("truncated end of central directory"))?;
    if disk != 0 || cd_disk != 0 {
        return Err(error("archives on several disks are not supported"));
    }
    if count == 0xFFFF || on_disk == 0xFFFF || cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF {
        return Err(error("ZIP64 archives are not supported"));
    }
    // Readers differ in which count they trust; a single-disk archive has one.
    if on_disk != count {
        return Err(error("end of central directory entry counts disagree"));
    }
    if usize::from(count) > options.max_entries {
        return Err(Error::new(ErrorKind::Limit, "package exceeds max_entries"));
    }

    let mut files = Vec::with_capacity(usize::from(count));
    let mut total: u64 = 0;
    let mut at = cd_offset as usize;
    // Byte ranges [local header, end of data) of every file entry.
    let mut ranges: Vec<(usize, usize, usize)> = Vec::with_capacity(usize::from(count));
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
        let name_bytes = bytes.get(at + 46..at + 46 + name_len).ok_or_else(truncated)?;
        let name = std::str::from_utf8(name_bytes).map_err(|_| error("non-UTF-8 entry names are not supported"))?.to_owned();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if u64::from(size) > options.max_entry_size {
            return Err(Error::new(ErrorKind::Limit, format!("{name}: entry exceeds max_entry_size")));
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
        let local_header = || error(format!("{name}: truncated local header"));
        let local_flags = u16_at(bytes, local + 6).ok_or_else(local_header)?;
        let local_method = u16_at(bytes, local + 8).ok_or_else(local_header)?;
        let local_crc = u32_at(bytes, local + 14).ok_or_else(local_header)?;
        let local_packed = u32_at(bytes, local + 18).ok_or_else(local_header)? as usize;
        let local_size = u32_at(bytes, local + 22).ok_or_else(local_header)?;
        let local_name = usize::from(u16_at(bytes, local + 26).ok_or_else(local_header)?);
        let local_extra = usize::from(u16_at(bytes, local + 28).ok_or_else(local_header)?);
        if bytes.get(local + 30..local + 30 + local_name) != Some(name_bytes) {
            return Err(error(format!("{name}: local header name does not match the central directory")));
        }
        if local_method != method {
            return Err(error(format!("{name}: local header compression method does not match the central directory")));
        }
        // Bit 3: CRC-32 and sizes follow the data in a descriptor instead.
        if local_flags & 0x0008 == 0 && (local_crc, local_packed, local_size) != (crc, packed, size) {
            return Err(error(format!("{name}: local header CRC-32 or sizes do not match the central directory")));
        }
        let start = local + 30 + local_name + local_extra;
        let raw = bytes.get(start..start.saturating_add(packed)).ok_or_else(|| error(format!("{name}: truncated data")))?;
        ranges.push((local, start + packed, files.len()));
        total = total.saturating_add(u64::from(size));
        if total > options.max_size {
            return Err(Error::new(ErrorKind::Limit, format!("package expands beyond {} bytes", options.max_size)));
        }
        let data = match compression {
            Compression::Stored => {
                // The declared expanded size was charged to the budget above.
                // Validate the packed length before allocating, otherwise a
                // malformed stored entry can bypass that budget temporarily.
                if raw.len() != size as usize {
                    return Err(error(format!("{name}: stored size does not match packed size")));
                }
                raw.to_vec()
            }
            Compression::Deflated => {
                miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size as usize).map_err(|e| error(format!("{name}: cannot inflate: {e}")))?
            }
        };
        if data.len() != size as usize || crc32(&data) != crc {
            return Err(error(format!("{name}: size or CRC-32 does not match")));
        }
        files.push(File { name, compression, data });
    }
    if Some(at) != (cd_offset as usize).checked_add(cd_size as usize) || at > end {
        return Err(error("central directory size does not match its entries"));
    }
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        if let [(_, end, first), (start, _, second)] = pair
            && start < end
        {
            let name = |i: &usize| files.get(*i).map_or("?", |f| f.name.as_str());
            return Err(error(format!("{} and {}: entries share archive bytes", name(first), name(second))));
        }
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

    fn read(bytes: &[u8], max_size: u64) -> Result<Vec<File>, Error> {
        super::read(bytes, &ReadOptions { max_size, ..ReadOptions::default() })
    }

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
        let decoded = read(&bytes, 1 << 20).unwrap();
        let names: Vec<(&str, Compression)> = decoded.iter().map(|f| (f.name.as_str(), f.compression)).collect();
        assert_eq!(names, [("mimetype", Compression::Stored), ("dir/ä.xml", Compression::Deflated)]);
        assert_eq!(decoded[1].data, text.as_bytes());
        assert!(matches!(read(&bytes, 100), Err(e) if e.kind() == ErrorKind::Limit));
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

    #[test]
    fn invalid_utf8_names_are_rejected_without_replacement() {
        let mut bytes = write(&[("a.xml", b"<a/>".as_slice(), Compression::Stored)]).unwrap();
        let central = bytes.windows(4).position(|b| b == CENTRAL.to_le_bytes()).unwrap();
        bytes[30] = 0xFF;
        bytes[central + 46] = 0xFF;
        assert!(matches!(read(&bytes, 1 << 20), Err(e) if e.kind() == ErrorKind::Zip && e.message().contains("non-UTF-8")));
    }

    #[test]
    fn stored_length_mismatch_is_rejected_before_copying() {
        let payload = vec![0x41; 1024 * 1024];
        let mut bytes = write(&[("large.bin", payload.as_slice(), Compression::Stored)]).unwrap();
        let central = bytes.windows(4).position(|b| b == CENTRAL.to_le_bytes()).unwrap();
        // The packed data is 1 MiB, while the attacker claims it expands to
        // one byte. It fits the configured budget only if that lie is trusted.
        // Both headers carry the lie, so they agree with each other.
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(read(&bytes, 1), Err(e) if e.kind() == ErrorKind::Zip && e.message().contains("stored size")));
    }

    fn central(bytes: &[u8]) -> usize {
        bytes.windows(4).position(|b| b == CENTRAL.to_le_bytes()).unwrap()
    }

    #[test]
    fn local_headers_must_agree_with_the_central_directory() {
        let original = write(&[("a.xml", b"<a/>".as_slice(), Compression::Stored)]).unwrap();
        assert!(read(&original, 1 << 20).is_ok());

        // Same-length different local name: readers that trust local headers
        // would see "b.xml", central-directory readers "a.xml".
        let mut renamed = original.clone();
        renamed[30] = b'b';
        assert!(matches!(read(&renamed, 1 << 20), Err(e) if e.message().contains("local header name")));

        let mut method = original.clone();
        method[8] = 8;
        assert!(matches!(read(&method, 1 << 20), Err(e) if e.message().contains("compression method")));

        // A local CRC that disagrees is rejected unless a data descriptor is declared.
        let mut crc = original.clone();
        crc[14] ^= 0xFF;
        assert!(matches!(read(&crc, 1 << 20), Err(e) if e.message().contains("CRC-32 or sizes")));
        crc[6] |= 0x08;
        crc[14..26].fill(0);
        assert_eq!(read(&crc, 1 << 20).unwrap()[0].data, b"<a/>");
    }

    #[test]
    fn the_end_record_comment_must_end_the_input() {
        let mut bytes = write(&[("a.xml", b"<a/>".as_slice(), Compression::Stored)]).unwrap();
        let len = bytes.len();
        // A comment that would extend past the input is truncated.
        bytes[len - 2] = 4;
        assert!(read(&bytes, 1 << 20).is_err());
        // A matching comment is accepted; trailing bytes after it are not.
        bytes.extend_from_slice(b"note");
        assert_eq!(read(&bytes, 1 << 20).unwrap().len(), 1);
        bytes.push(0);
        assert!(read(&bytes, 1 << 20).is_err());
    }

    #[test]
    fn entries_cannot_share_archive_bytes() {
        let bytes = write(&[("a.xml", b"<a/>".as_slice(), Compression::Stored), ("b.xml", b"<b/>".as_slice(), Compression::Stored)]).unwrap();
        let first = central(&bytes);
        let second = first + 46 + "a.xml".len();
        // Point the second central entry at the first local header, renamed to match.
        let mut aliased = bytes.clone();
        aliased[second + 42..second + 46].copy_from_slice(&0u32.to_le_bytes());
        aliased[second + 46..second + 51].copy_from_slice(b"a.xml");
        aliased[second + 16..second + 20].copy_from_slice(&bytes[first + 16..first + 20]);
        assert!(matches!(read(&aliased, 1 << 20), Err(e) if e.message().contains("share archive bytes")), "{:?}", read(&aliased, 1 << 20).err());
    }

    #[test]
    fn written_archives_match_an_independent_reader() {
        use std::io::Read;
        let text = "<a>ä</a>".repeat(500);
        let files = [("mimetype", b"application/x".as_slice(), Compression::Stored), ("dir/b.xml", text.as_bytes(), Compression::Deflated)];
        let bytes = write(&files).unwrap();
        let mut other = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        for file in read(&bytes, 1 << 20).unwrap() {
            let mut data = Vec::new();
            other.by_name(&file.name).unwrap().read_to_end(&mut data).unwrap();
            assert_eq!(data, file.data);
        }
    }

    #[test]
    fn entry_limits_are_checked_before_reading_content() {
        let files: Vec<_> = (0..50).map(|i| (format!("{i}.txt"), Vec::<u8>::new())).collect();
        let input: Vec<_> = files.iter().map(|(name, data)| (name.as_str(), data.as_slice(), Compression::Stored)).collect();
        let bytes = write(&input).unwrap();
        let options = ReadOptions { max_entries: 49, ..ReadOptions::default() };
        assert!(matches!(super::read(&bytes, &options), Err(e) if e.kind() == ErrorKind::Limit && e.message().contains("max_entries")));
        let options = ReadOptions { max_entries: 50, ..options };
        assert_eq!(super::read(&bytes, &options).unwrap().len(), 50);

        for compression in [Compression::Stored, Compression::Deflated] {
            let bytes = write(&[("file", b"12345".as_slice(), compression)]).unwrap();
            let options = ReadOptions { max_entry_size: 4, ..ReadOptions::default() };
            assert!(matches!(super::read(&bytes, &options), Err(e) if e.kind() == ErrorKind::Limit && e.message().contains("max_entry_size")));
            let options = ReadOptions { max_entry_size: 5, ..options };
            assert_eq!(super::read(&bytes, &options).unwrap()[0].data, b"12345");
        }
    }
}
