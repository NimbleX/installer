//! Raw NTFS `$Bitmap` reader — works on dirty/hibernated volumes.
//!
//! When Windows is suspended via Fast Startup, the NTFS volume is left in a
//! "dirty" state: `$MFTMirr` and `$MFT` disagree, and every standard tool
//! (`ntfsresize`, `ntfs-3g`, `ntfsinfo`, `ntfscluster`) refuses to read it
//! without first running `chkdsk /f` from Windows or `ntfsfix -d` (which
//! mutates the disk). For a *display-only* used-space probe we don't need
//! the journal or the mirror — we only need the boot sector (always intact)
//! and MFT record 6 (`$Bitmap`), which gives us the cluster allocation
//! bitmap directly.
//!
//! Caveats:
//! * Requires read access to the raw block device (root, or member of `disk`).
//! * Returns `None` on anything unexpected — no panics, no writes, ever.
//! * NTFS fixups are applied to the MFT record so the parser is correct
//!   on volumes with cluster size ≥ 1 KiB and MFT records spanning multiple
//!   sectors.

use crate::size::Bytes;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Read the cluster-allocation bitmap from `$Bitmap` and return the
/// number of used bytes. Read-only; safe on dirty volumes.
pub fn probe_used(dev: &Path) -> Option<Bytes> {
    let mut f = File::open(dev).ok()?;

    // ---- Boot sector --------------------------------------------------
    let mut boot = [0u8; 512];
    f.read_exact(&mut boot).ok()?;
    if &boot[3..11] != b"NTFS    " {
        return None;
    }
    let bytes_per_sector = u16::from_le_bytes([boot[0x0B], boot[0x0C]]) as u64;
    let sectors_per_cluster = boot[0x0D] as u64;
    if bytes_per_sector == 0 || sectors_per_cluster == 0 {
        return None;
    }
    let cluster_size = bytes_per_sector.checked_mul(sectors_per_cluster)?;
    let total_sectors = u64::from_le_bytes(boot[0x28..0x30].try_into().ok()?);
    let mft_lcn = u64::from_le_bytes(boot[0x30..0x38].try_into().ok()?);
    let cpr = boot[0x40] as i8;
    let mft_record_size: u64 = if cpr >= 0 {
        (cpr as u64).checked_mul(cluster_size)?
    } else {
        let shift = -(cpr as i32) as u32;
        if shift >= 32 {
            return None;
        }
        1u64 << shift
    };
    if !(256..=65536).contains(&mft_record_size) {
        return None;
    }

    // ---- Read MFT record 6 ($Bitmap) ----------------------------------
    let mft_offset = mft_lcn.checked_mul(cluster_size)?;
    let bitmap_rec_offset = mft_offset.checked_add(6u64.checked_mul(mft_record_size)?)?;
    let mut record = vec![0u8; mft_record_size as usize];
    f.seek(SeekFrom::Start(bitmap_rec_offset)).ok()?;
    f.read_exact(&mut record).ok()?;
    if &record[0..4] != b"FILE" {
        return None;
    }
    apply_fixups(&mut record, bytes_per_sector as usize)?;

    // ---- Walk attributes, find non-resident $DATA ---------------------
    let first_attr = u16::from_le_bytes([record[0x14], record[0x15]]) as usize;
    let mut off = first_attr;
    let runlist_slice;
    let alloc_size;
    loop {
        if off + 4 > record.len() {
            return None;
        }
        let attr_type = u32::from_le_bytes(record[off..off + 4].try_into().ok()?);
        if attr_type == ATTR_END {
            return None;
        }
        if off + 8 > record.len() {
            return None;
        }
        let attr_len = u32::from_le_bytes(record[off + 4..off + 8].try_into().ok()?) as usize;
        if attr_len == 0 || off + attr_len > record.len() {
            return None;
        }
        if attr_type == ATTR_DATA {
            let non_resident = record[off + 8] != 0;
            if !non_resident {
                return None; // $Bitmap should always be non-resident
            }
            // Non-resident header layout (offsets within attribute):
            //   0x20 alloc size (u64), 0x28 real size (u64),
            //   0x20 mapping_pairs_offset (u16) at offset 0x20? No:
            //   actually: 0x20 mapping_pairs_offset is at 0x20 within attr.
            //   Standard layout:
            //     0x10 starting_vcn  u64
            //     0x18 last_vcn      u64
            //     0x20 mapping_pairs_offset u16
            //     0x22 compression_unit u16
            //     0x28 allocated_size u64
            //     0x30 real_size      u64
            //     0x38 initialized_size u64
            let mp_off =
                u16::from_le_bytes(record[off + 0x20..off + 0x22].try_into().ok()?) as usize;
            alloc_size =
                u64::from_le_bytes(record[off + 0x28..off + 0x30].try_into().ok()?);
            let rl_start = off + mp_off;
            let rl_end = off + attr_len;
            if rl_start >= rl_end {
                return None;
            }
            runlist_slice = record[rl_start..rl_end].to_vec();
            break;
        }
        off += attr_len;
    }

    // ---- Decode runlist & popcount the bitmap -------------------------
    let mut used_clusters: u64 = 0;
    let mut prev_lcn: i64 = 0;
    let mut bytes_remaining = alloc_size;
    let mut i = 0;
    while i < runlist_slice.len() && runlist_slice[i] != 0 && bytes_remaining > 0 {
        let header = runlist_slice[i];
        let len_size = (header & 0x0F) as usize;
        let off_size = ((header >> 4) & 0x0F) as usize;
        i += 1;
        if len_size == 0 || i + len_size + off_size > runlist_slice.len() {
            return None;
        }
        let length = read_le_unsigned(&runlist_slice[i..i + len_size]);
        i += len_size;

        let mut run_bytes = length.checked_mul(cluster_size)?;
        if run_bytes > bytes_remaining {
            run_bytes = bytes_remaining;
        }
        bytes_remaining -= run_bytes;

        if off_size == 0 {
            // Sparse run: bits all zero. Skip.
            continue;
        }
        let delta = read_le_signed(&runlist_slice[i..i + off_size]);
        i += off_size;
        prev_lcn = prev_lcn.checked_add(delta)?;
        if prev_lcn < 0 {
            return None;
        }

        let run_disk_offset = (prev_lcn as u64).checked_mul(cluster_size)?;
        f.seek(SeekFrom::Start(run_disk_offset)).ok()?;

        // Stream the run in chunks so we don't allocate hundreds of MB.
        let mut left = run_bytes;
        let mut buf = vec![0u8; 1024 * 1024];
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..want]).ok()?;
            for &b in &buf[..want] {
                used_clusters += b.count_ones() as u64;
            }
            left -= want as u64;
        }
    }

    let total_clusters = total_sectors / sectors_per_cluster;
    let used_clusters = used_clusters.min(total_clusters);
    Some(Bytes(used_clusters.checked_mul(cluster_size)?))
}

/// Apply the NTFS Update Sequence Number fixup to a multi-sector record.
/// The last 2 bytes of every sector are placeholders; the real values are
/// stored in the USN array at the start of the record. Returns `None` if
/// the record's USN doesn't match (corruption).
fn apply_fixups(record: &mut [u8], sector_size: usize) -> Option<()> {
    if record.len() < 8 || sector_size < 2 {
        return None;
    }
    let usa_off = u16::from_le_bytes([record[4], record[5]]) as usize;
    let usa_count = u16::from_le_bytes([record[6], record[7]]) as usize; // includes USN itself
    if usa_count == 0 || usa_off + usa_count * 2 > record.len() {
        return None;
    }
    let usn = [record[usa_off], record[usa_off + 1]];
    for i in 1..usa_count {
        let sector_end = i * sector_size;
        if sector_end < 2 || sector_end > record.len() {
            return None;
        }
        // Verify placeholder matches USN
        if record[sector_end - 2] != usn[0] || record[sector_end - 1] != usn[1] {
            return None;
        }
        let arr_pos = usa_off + i * 2;
        record[sector_end - 2] = record[arr_pos];
        record[sector_end - 1] = record[arr_pos + 1];
    }
    Some(())
}

fn read_le_unsigned(bytes: &[u8]) -> u64 {
    let mut v: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        v |= (b as u64) << (i * 8);
    }
    v
}

fn read_le_signed(bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let mut v: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        v |= (b as u64) << (i * 8);
    }
    // Sign-extend from the top bit of the highest byte present.
    let bits = bytes.len() * 8;
    let sign_bit = 1u64 << (bits - 1);
    if v & sign_bit != 0 {
        let mask = !((1u64 << bits) - 1);
        (v | mask) as i64
    } else {
        v as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_extension_negative() {
        // 0xFE in 1 byte == -2
        assert_eq!(read_le_signed(&[0xFE]), -2);
        // 0xFFFE in 2 bytes == -2
        assert_eq!(read_le_signed(&[0xFE, 0xFF]), -2);
    }

    #[test]
    fn signed_extension_positive() {
        assert_eq!(read_le_signed(&[0x05]), 5);
        assert_eq!(read_le_signed(&[0x05, 0x00]), 5);
    }

    #[test]
    fn unsigned_le() {
        assert_eq!(read_le_unsigned(&[0x34, 0x12]), 0x1234);
        assert_eq!(read_le_unsigned(&[0x01, 0x00, 0x00, 0x80]), 0x80000001);
    }
}
