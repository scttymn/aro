//! Android sparse image format (`system/core/libsparse/sparse_format.h`).
//!
//! A sparse image is a header followed by chunks. Each chunk is either raw
//! data, a 4-byte fill pattern repeated, a "don't care" hole, or a CRC record.
//! We expand it to a raw image, writing holes as holes so the output stays
//! sparse on disk.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

pub const MAGIC: u32 = 0xed26_ff3a;
const CHUNK_RAW: u16 = 0xCAC1;
const CHUNK_FILL: u16 = 0xCAC2;
const CHUNK_DONT_CARE: u16 = 0xCAC3;
const CHUNK_CRC32: u16 = 0xCAC4;

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// True if the file starts with the sparse magic.
pub fn is_sparse(f: &mut File) -> Result<bool> {
    let mut m = [0u8; 4];
    f.seek(SeekFrom::Start(0))?;
    if f.read(&mut m)? < 4 {
        return Ok(false);
    }
    Ok(u32::from_le_bytes(m) == MAGIC)
}

/// Expand `input` (sparse) into `output` (raw). Returns the raw size in bytes.
pub fn expand(input: &mut File, output: &mut File) -> Result<u64> {
    input.seek(SeekFrom::Start(0))?;
    let mut h = [0u8; 28];
    input.read_exact(&mut h).context("reading sparse header")?;
    if u32_at(&h, 0) != MAGIC {
        bail!("not a sparse image");
    }
    let file_hdr_sz = u16_at(&h, 8) as u64;
    let chunk_hdr_sz = u16_at(&h, 10) as usize;
    let blk_sz = u32_at(&h, 12) as u64;
    let total_blks = u32_at(&h, 16) as u64;
    let total_chunks = u32_at(&h, 20);
    if chunk_hdr_sz < 12 {
        bail!("bad chunk header size {chunk_hdr_sz}");
    }
    input.seek(SeekFrom::Start(file_hdr_sz))?;

    let mut out_pos: u64 = 0;
    let mut chunk_hdr = vec![0u8; chunk_hdr_sz];
    let mut buf = vec![0u8; 1 << 20];
    for _ in 0..total_chunks {
        input.read_exact(&mut chunk_hdr).context("reading chunk header")?;
        let ty = u16_at(&chunk_hdr, 0);
        let n_blocks = u32_at(&chunk_hdr, 4) as u64;
        let total_sz = u32_at(&chunk_hdr, 8) as u64;
        let data_len = total_sz - chunk_hdr_sz as u64;
        let span = n_blocks * blk_sz;
        match ty {
            CHUNK_RAW => {
                if data_len != span {
                    bail!("raw chunk size mismatch");
                }
                output.seek(SeekFrom::Start(out_pos))?;
                let mut left = span;
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    input.read_exact(&mut buf[..n])?;
                    output.write_all(&buf[..n])?;
                    left -= n as u64;
                }
            }
            CHUNK_FILL => {
                let mut fill = [0u8; 4];
                input.read_exact(&mut fill)?;
                if fill != [0, 0, 0, 0] {
                    output.seek(SeekFrom::Start(out_pos))?;
                    for c in buf.chunks_exact_mut(4) {
                        c.copy_from_slice(&fill);
                    }
                    let mut left = span;
                    while left > 0 {
                        let n = left.min(buf.len() as u64) as usize;
                        output.write_all(&buf[..n])?;
                        left -= n as u64;
                    }
                }
            }
            CHUNK_DONT_CARE => {}
            CHUNK_CRC32 => {
                input.seek(SeekFrom::Current(data_len as i64))?;
                continue;
            }
            other => bail!("unknown chunk type {other:#x}"),
        }
        out_pos += span;
    }
    let raw_size = total_blks * blk_sz;
    output.set_len(raw_size)?;
    Ok(raw_size)
}
