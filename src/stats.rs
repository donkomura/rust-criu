use crate::rust_criu_protobuf::stats::{Dump_stats_entry, Restore_stats_entry, Stats_entry};
use crate::CriuError;
use protobuf::Message;
use std::path::Path;

pub const STATS_DUMP: &str = "stats-dump";
pub const STATS_RESTORE: &str = "stats-restore";

// CRIU writes a stats file as IMG_SERVICE_MAGIC, STATS_MAGIC, the payload size
// (all little-endian u32), and then one stats_entry message.
// https://github.com/checkpoint-restore/criu/blob/v4.2/criu/include/magic.h
const IMG_SERVICE_MAGIC: u32 = 0x55105940;
const STATS_MAGIC: u32 = 0x57093306;

const PRIMARY_MAGIC_OFFSET: usize = 0;
const SECONDARY_MAGIC_OFFSET: usize = 4;
const SIZE_OFFSET: usize = 8;
const PAYLOAD_OFFSET: usize = 12;

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn parse_stats(buf: &[u8]) -> Result<Stats_entry, CriuError> {
    if buf.len() < PAYLOAD_OFFSET {
        return Err(CriuError::InvalidStatsImage(format!(
            "image is {} bytes, shorter than the {} byte header",
            buf.len(),
            PAYLOAD_OFFSET
        )));
    }
    if read_u32_le(buf, PRIMARY_MAGIC_OFFSET) != IMG_SERVICE_MAGIC {
        return Err(CriuError::InvalidStatsImage(
            "primary magic not found".to_string(),
        ));
    }
    if read_u32_le(buf, SECONDARY_MAGIC_OFFSET) != STATS_MAGIC {
        return Err(CriuError::InvalidStatsImage(
            "secondary magic not found".to_string(),
        ));
    }

    let payload_size = read_u32_le(buf, SIZE_OFFSET) as usize;
    let payload = buf
        .get(PAYLOAD_OFFSET..PAYLOAD_OFFSET + payload_size)
        .ok_or_else(|| {
            CriuError::InvalidStatsImage(format!(
                "payload of {} bytes is truncated to {} bytes",
                payload_size,
                buf.len() - PAYLOAD_OFFSET
            ))
        })?;
    Ok(Stats_entry::parse_from_bytes(payload)?)
}

fn read_stats_file(images_dir: &Path, file_name: &str) -> Result<Stats_entry, CriuError> {
    parse_stats(&std::fs::read(images_dir.join(file_name))?)
}

/// Reads the statistics that CRIU wrote to `stats-dump` in `images_dir` after a dump.
pub fn get_dump_stats(images_dir: &Path) -> Result<Dump_stats_entry, CriuError> {
    read_stats_file(images_dir, STATS_DUMP)?
        .dump
        .into_option()
        .ok_or_else(|| CriuError::InvalidStatsImage(format!("{} has no dump stats", STATS_DUMP)))
}

/// Reads the statistics that CRIU wrote to `stats-restore` in `images_dir` after a restore.
pub fn get_restore_stats(images_dir: &Path) -> Result<Restore_stats_entry, CriuError> {
    read_stats_file(images_dir, STATS_RESTORE)?
        .restore
        .into_option()
        .ok_or_else(|| {
            CriuError::InvalidStatsImage(format!("{} has no restore stats", STATS_RESTORE))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dump_stats_entry() -> Dump_stats_entry {
        let mut dump = Dump_stats_entry::new();
        dump.set_freezing_time(1);
        dump.set_frozen_time(2);
        dump.set_memdump_time(3);
        dump.set_memwrite_time(4);
        dump.set_pages_scanned(43);
        dump.set_pages_skipped_parent(18);
        dump.set_pages_written(10);
        dump.set_pages_lazy(0);
        dump
    }

    fn restore_stats_entry() -> Restore_stats_entry {
        let mut restore = Restore_stats_entry::new();
        restore.set_pages_compared(5);
        restore.set_pages_skipped_cow(6);
        restore.set_forking_time(7);
        restore.set_restore_time(8);
        restore
    }

    fn stats_image(primary_magic: u32, secondary_magic: u32, entry: &Stats_entry) -> Vec<u8> {
        let payload = entry.write_to_bytes().unwrap();
        let mut image = Vec::new();
        image.extend_from_slice(&primary_magic.to_le_bytes());
        image.extend_from_slice(&secondary_magic.to_le_bytes());
        image.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        image.extend_from_slice(&payload);
        image
    }

    fn dump_stats_image() -> Vec<u8> {
        let mut entry = Stats_entry::new();
        entry.dump = Some(dump_stats_entry()).into();
        stats_image(IMG_SERVICE_MAGIC, STATS_MAGIC, &entry)
    }

    fn restore_stats_image() -> Vec<u8> {
        let mut entry = Stats_entry::new();
        entry.restore = Some(restore_stats_entry()).into();
        stats_image(IMG_SERVICE_MAGIC, STATS_MAGIC, &entry)
    }

    fn images_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rust-criu-stats-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_stats_reads_dump_entry() {
        let entry = parse_stats(&dump_stats_image()).unwrap();
        assert_eq!(entry.dump.pages_skipped_parent(), 18);
        assert_eq!(entry.dump.pages_written(), 10);
    }

    #[test]
    fn parse_stats_rejects_wrong_primary_magic() {
        let mut entry = Stats_entry::new();
        entry.dump = Some(dump_stats_entry()).into();
        let image = stats_image(0, STATS_MAGIC, &entry);
        assert!(matches!(
            parse_stats(&image),
            Err(CriuError::InvalidStatsImage(_))
        ));
    }

    #[test]
    fn parse_stats_rejects_wrong_secondary_magic() {
        let mut entry = Stats_entry::new();
        entry.dump = Some(dump_stats_entry()).into();
        let image = stats_image(IMG_SERVICE_MAGIC, 0, &entry);
        assert!(matches!(
            parse_stats(&image),
            Err(CriuError::InvalidStatsImage(_))
        ));
    }

    #[test]
    fn parse_stats_rejects_image_shorter_than_header() {
        let image = dump_stats_image();
        assert!(matches!(
            parse_stats(&image[..PAYLOAD_OFFSET - 1]),
            Err(CriuError::InvalidStatsImage(_))
        ));
    }

    #[test]
    fn parse_stats_rejects_truncated_payload() {
        let image = dump_stats_image();
        assert!(matches!(
            parse_stats(&image[..image.len() - 1]),
            Err(CriuError::InvalidStatsImage(_))
        ));
    }

    #[test]
    fn get_dump_stats_reads_stats_dump_file() {
        let dir = images_dir("dump");
        std::fs::write(dir.join(STATS_DUMP), dump_stats_image()).unwrap();

        let dump = get_dump_stats(&dir).unwrap();
        assert_eq!(dump.pages_skipped_parent(), 18);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn get_dump_stats_rejects_entry_without_dump_stats() {
        let dir = images_dir("dump-missing");
        std::fs::write(dir.join(STATS_DUMP), restore_stats_image()).unwrap();

        assert!(matches!(
            get_dump_stats(&dir),
            Err(CriuError::InvalidStatsImage(_))
        ));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn get_restore_stats_reads_stats_restore_file() {
        let dir = images_dir("restore");
        std::fs::write(dir.join(STATS_RESTORE), restore_stats_image()).unwrap();

        let restore = get_restore_stats(&dir).unwrap();
        assert_eq!(restore.pages_compared(), 5);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn get_dump_stats_returns_io_error_when_file_is_missing() {
        let dir = images_dir("no-file");

        assert!(matches!(get_dump_stats(&dir), Err(CriuError::Io(_))));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
