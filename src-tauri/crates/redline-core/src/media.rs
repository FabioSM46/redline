//! The length of a saved video, read from its container header without decoding anything and
//! without extra tools: Matroska/WebM (`Segment → Info → Duration`) and MP4 (`moov → mvhd`).
//! Anything else, or a header without a usable duration, is "unknown" (`None`), never an error.
//!
//! Only a few seeks and small reads: the clusters and the media data are skipped by size.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// EBML header, Segment, Info, Cluster, TimestampScale and Duration element IDs (Matroska).
const EBML_HEADER: u64 = 0x1A45_DFA3;
const MKV_SEGMENT: u64 = 0x1853_8067;
const MKV_INFO: u64 = 0x1549_A966;
const MKV_CLUSTER: u64 = 0x1F43_B675;
const MKV_TIMESTAMP_SCALE: u64 = 0x2A_D7B1;
const MKV_DURATION: u64 = 0x4489;
/// Matroska's default timestamp scale: one tick is a millisecond.
const MKV_DEFAULT_SCALE: u64 = 1_000_000;
/// Larger `Info` elements are not read: real ones are a few hundred bytes.
const MAX_INFO_BYTES: u64 = 64 * 1024;
/// How many top-level elements or boxes are looked at before giving up.
const MAX_ELEMENTS: usize = 256;

/// The duration of the video at `path` in seconds, or `None` when it cannot be read from the
/// header (another container, a file still being written, a damaged file).
pub fn duration_seconds(path: &Path) -> Option<f64> {
    let mut file = File::open(path).ok()?;
    duration_of(&mut file)
}

/// [`duration_seconds`] on any seekable source; the container is recognised by its first bytes.
pub fn duration_of<R: Read + Seek>(reader: &mut R) -> Option<f64> {
    let len = reader.seek(SeekFrom::End(0)).ok()?;
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut head = [0u8; 8];
    reader.read_exact(&mut head).ok()?;
    reader.seek(SeekFrom::Start(0)).ok()?;
    let seconds = if head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        matroska_duration(reader, len).ok()?
    } else if &head[4..8] == b"ftyp" {
        mp4_duration(reader, len).ok()?
    } else {
        None
    }?;
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unexpected container layout")
}

fn read_byte(reader: &mut impl Read) -> io::Result<u8> {
    let mut byte = [0u8; 1];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

/// An EBML variable-length integer: the element ID with its length marker kept, or a size with
/// the marker removed. The flag says whether every value bit is set ("unknown size").
fn ebml_vint(reader: &mut impl Read, keep_marker: bool) -> io::Result<(u64, bool)> {
    let first = read_byte(reader)?;
    let len = first.leading_zeros() + 1;
    if len > 8 {
        return Err(invalid());
    }
    let mask = 0xFFu8.checked_shr(len).unwrap_or(0);
    let mut all_ones = first & mask == mask;
    let mut value = u64::from(if keep_marker { first } else { first & mask });
    for _ in 1..len {
        let byte = read_byte(reader)?;
        all_ones &= byte == 0xFF;
        value = (value << 8) | u64::from(byte);
    }
    Ok((value, all_ones))
}

/// One EBML element header: its ID, where its data starts and where it ends (`None` for an
/// unknown size).
fn ebml_element<R: Read + Seek>(reader: &mut R) -> io::Result<(u64, u64, Option<u64>)> {
    let (id, _) = ebml_vint(reader, true)?;
    let (size, unknown) = ebml_vint(reader, false)?;
    let start = reader.stream_position()?;
    let end = if unknown {
        None
    } else {
        Some(start.checked_add(size).ok_or_else(invalid)?)
    };
    Ok((id, start, end))
}

fn matroska_duration<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<Option<f64>> {
    let (id, _, end) = ebml_element(reader)?;
    if id != EBML_HEADER {
        return Ok(None);
    }
    reader.seek(SeekFrom::Start(end.ok_or_else(invalid)?))?;
    let (id, _, segment_end) = ebml_element(reader)?;
    if id != MKV_SEGMENT {
        return Ok(None);
    }
    let segment_end = segment_end.unwrap_or(len).min(len);
    for _ in 0..MAX_ELEMENTS {
        if reader.stream_position()? >= segment_end {
            break;
        }
        let (id, start, end) = ebml_element(reader)?;
        match (id, end) {
            (MKV_INFO, Some(end)) if end - start <= MAX_INFO_BYTES => {
                let mut info = vec![0u8; usize::try_from(end - start).map_err(|_| invalid())?];
                reader.read_exact(&mut info)?;
                return Ok(matroska_info_duration(&info));
            }
            // Info comes before the media: past the first cluster there is nothing to find.
            (MKV_CLUSTER, _) | (_, None) => break,
            (_, Some(end)) => {
                reader.seek(SeekFrom::Start(end))?;
            }
        }
    }
    Ok(None)
}

/// `Duration × TimestampScale` from the body of an `Info` element, in seconds.
fn matroska_info_duration(info: &[u8]) -> Option<f64> {
    let mut reader = io::Cursor::new(info);
    let mut scale = MKV_DEFAULT_SCALE;
    let mut duration = None;
    while reader.position() < info.len() as u64 {
        let (id, start, end) = ebml_element(&mut reader).ok()?;
        let end = end?;
        let data = info.get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)?;
        match id {
            MKV_TIMESTAMP_SCALE => scale = big_endian(data)?,
            MKV_DURATION => {
                duration = match data.len() {
                    4 => Some(f64::from(f32::from_be_bytes(data.try_into().ok()?))),
                    8 => Some(f64::from_be_bytes(data.try_into().ok()?)),
                    _ => None,
                }
            }
            _ => {}
        }
        reader.set_position(end);
    }
    duration.map(|ticks| ticks * scale as f64 / 1e9)
}

fn big_endian(data: &[u8]) -> Option<u64> {
    if data.is_empty() || data.len() > 8 {
        return None;
    }
    Some(
        data.iter()
            .fold(0u64, |value, byte| (value << 8) | u64::from(*byte)),
    )
}

/// One MP4 box header: its type, where its payload starts and where it ends.
fn mp4_box<R: Read + Seek>(reader: &mut R, limit: u64) -> io::Result<([u8; 4], u64, u64)> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;
    let size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let kind = [header[4], header[5], header[6], header[7]];
    let box_start = reader.stream_position()? - 8;
    let size = match size {
        0 => limit - box_start,
        1 => {
            let mut large = [0u8; 8];
            reader.read_exact(&mut large)?;
            u64::from_be_bytes(large)
        }
        size => size,
    };
    let start = reader.stream_position()?;
    let end = box_start.checked_add(size).ok_or_else(invalid)?;
    if end < start || end > limit {
        return Err(invalid());
    }
    Ok((kind, start, end))
}

/// Finds the box `kind` among the boxes between the current position and `limit`.
fn mp4_find<R: Read + Seek>(
    reader: &mut R,
    kind: &[u8; 4],
    limit: u64,
) -> io::Result<Option<(u64, u64)>> {
    for _ in 0..MAX_ELEMENTS {
        if reader.stream_position()? + 8 > limit {
            break;
        }
        let (found, start, end) = mp4_box(reader, limit)?;
        if &found == kind {
            return Ok(Some((start, end)));
        }
        reader.seek(SeekFrom::Start(end))?;
    }
    Ok(None)
}

fn mp4_duration<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<Option<f64>> {
    let Some((_, moov_end)) = mp4_find(reader, b"moov", len)? else {
        return Ok(None);
    };
    let Some((start, end)) = mp4_find(reader, b"mvhd", moov_end)? else {
        return Ok(None);
    };
    let mut payload = vec![0u8; usize::try_from((end - start).min(128)).map_err(|_| invalid())?];
    reader.read_exact(&mut payload)?;
    Ok(mvhd_duration(&payload))
}

/// `duration / timescale` from an `mvhd` payload (version 0 or 1), in seconds. A zero or
/// all-ones duration (fragmented or unknown) is `None`.
fn mvhd_duration(payload: &[u8]) -> Option<f64> {
    let (timescale, duration) = match payload.first()? {
        0 => (
            big_endian(payload.get(12..16)?)?,
            big_endian(payload.get(16..20)?)?,
        ),
        1 => (
            big_endian(payload.get(20..24)?)?,
            big_endian(payload.get(24..32)?)?,
        ),
        _ => return None,
    };
    let unknown = duration == 0 || duration == u64::from(u32::MAX) || duration == u64::MAX;
    (timescale != 0 && !unknown).then(|| duration as f64 / timescale as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// An EBML element with an 8-byte size field, as muxers write the big ones.
    fn ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    /// An EBML element with a one-byte size field.
    fn ebml_short(id: &[u8], body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 0x7F);
        let mut out = id.to_vec();
        out.push(0x80 | body.len() as u8);
        out.extend_from_slice(body);
        out
    }

    fn matroska(segment_children: &[Vec<u8>], unknown_segment_size: bool) -> Vec<u8> {
        let mut out = ebml_short(
            &[0x1A, 0x45, 0xDF, 0xA3],
            &ebml_short(&[0x42, 0x82], b"matroska"),
        );
        let body: Vec<u8> = segment_children.concat();
        if unknown_segment_size {
            out.extend_from_slice(&[
                0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            ]);
            out.extend_from_slice(&body);
        } else {
            out.extend(ebml(&[0x18, 0x53, 0x80, 0x67], &body));
        }
        out
    }

    fn info(scale: Option<u32>, duration: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        if let Some(scale) = scale {
            body.extend(ebml_short(&[0x2A, 0xD7, 0xB1], &scale.to_be_bytes()));
        }
        body.extend(ebml_short(&[0x4D, 0x80], b"Lavf"));
        body.extend(ebml_short(&[0x44, 0x89], duration));
        ebml(&[0x15, 0x49, 0xA9, 0x66], &body)
    }

    fn seek_head() -> Vec<u8> {
        ebml(&[0x11, 0x4D, 0x9B, 0x74], &[0xEC, 0x82, 0x00, 0x00])
    }

    fn cluster() -> Vec<u8> {
        ebml(&[0x1F, 0x43, 0xB6, 0x75], &[0u8; 64])
    }

    fn duration(bytes: Vec<u8>) -> Option<f64> {
        duration_of(&mut Cursor::new(bytes))
    }

    #[test]
    fn matroska_duration_is_read_after_the_seek_head() {
        let file = matroska(
            &[
                seek_head(),
                info(Some(1_000_000), &60_500.0f64.to_be_bytes()),
                cluster(),
            ],
            false,
        );
        assert_eq!(duration(file), Some(60.5));
    }

    #[test]
    fn matroska_uses_the_default_scale_and_four_byte_floats() {
        let file = matroska(&[info(None, &1_500.0f32.to_be_bytes())], false);
        assert_eq!(duration(file), Some(1.5));
        let file = matroska(&[info(Some(1_000), &2_000_000.0f64.to_be_bytes())], true);
        assert_eq!(duration(file), Some(2.0));
    }

    #[test]
    fn matroska_without_a_duration_before_the_media_is_unknown() {
        // Info after the first cluster is not looked for.
        let file = matroska(&[cluster(), info(None, &1_000.0f64.to_be_bytes())], false);
        assert_eq!(duration(file), None);
        // A zero duration (a file whose writer never finished) is unknown, not zero.
        let file = matroska(&[info(None, &0.0f64.to_be_bytes())], false);
        assert_eq!(duration(file), None);
        // A file cut in the middle of Info.
        let mut file = matroska(&[seek_head(), info(None, &1_000.0f64.to_be_bytes())], false);
        file.truncate(file.len() - 4);
        assert_eq!(duration(file), None);
    }

    fn mp4_box_bytes(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0u8; 4 + 8];
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 80]);
        mp4_box_bytes(b"mvhd", &body)
    }

    fn mvhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut body = vec![1u8, 0, 0, 0];
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 80]);
        mp4_box_bytes(b"mvhd", &body)
    }

    fn mp4(moov_children: &[Vec<u8>], moov_last: bool) -> Vec<u8> {
        let ftyp = mp4_box_bytes(b"ftyp", b"isom\0\0\x02\0isomiso2mp41");
        let mdat = mp4_box_bytes(b"mdat", &[0u8; 128]);
        let moov = mp4_box_bytes(b"moov", &moov_children.concat());
        if moov_last {
            [ftyp, mdat, moov].concat()
        } else {
            [ftyp, moov, mdat].concat()
        }
    }

    #[test]
    fn mp4_duration_is_read_wherever_the_movie_header_is() {
        let trak = mp4_box_bytes(b"trak", &[0u8; 16]);
        assert_eq!(duration(mp4(&[mvhd_v0(1_000, 30_250)], true)), Some(30.25));
        assert_eq!(
            duration(mp4(&[trak, mvhd_v1(90_000, 900_000)], false)),
            Some(10.0)
        );
    }

    #[test]
    fn mp4_without_a_usable_duration_is_unknown() {
        // Fragmented files leave the movie header's duration at zero.
        assert_eq!(duration(mp4(&[mvhd_v0(1_000, 0)], false)), None);
        assert_eq!(duration(mp4(&[mvhd_v0(1_000, u32::MAX)], false)), None);
        assert_eq!(duration(mp4(&[mvhd_v0(0, 1_000)], false)), None);
        // No moov at all (still being written).
        let ftyp = mp4_box_bytes(b"ftyp", b"isom\0\0\x02\0");
        assert_eq!(
            duration([ftyp.clone(), mp4_box_bytes(b"mdat", &[0; 8])].concat()),
            None
        );
        // A box that claims more bytes than the file has.
        let mut broken = ftyp;
        broken.extend_from_slice(&[0, 0, 0x10, 0, b'm', b'o', b'o', b'v']);
        assert_eq!(duration(broken), None);
    }

    #[test]
    fn other_files_have_no_duration() {
        assert_eq!(duration(b"FLV\x01\x05\0\0\0\x09".to_vec()), None);
        assert_eq!(duration(Vec::new()), None);
        assert_eq!(
            duration_seconds(Path::new("/nonexistent/redline/clip.mkv")),
            None
        );
    }

    #[test]
    fn ebml_integers_keep_or_drop_the_marker() {
        let mut reader = Cursor::new([0x1A, 0x45, 0xDF, 0xA3]);
        assert_eq!(ebml_vint(&mut reader, true).unwrap(), (EBML_HEADER, false));
        let mut reader = Cursor::new([0x40, 0x02]);
        assert_eq!(ebml_vint(&mut reader, false).unwrap(), (2, false));
        let mut reader = Cursor::new([0xFF]);
        assert_eq!(ebml_vint(&mut reader, false).unwrap(), (0x7F, true));
        let mut reader = Cursor::new([0x00]);
        assert!(ebml_vint(&mut reader, false).is_err());
    }
}
