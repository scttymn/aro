//! Shared CursorWindow and BulkCursor helpers for ContentProviders.

use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use super::Service;

pub struct BulkCursorService;

impl Service for BulkCursorService {
    const DESCRIPTOR: &'static str = "android.content.IBulkCursor";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "GET_CURSOR_WINDOW"),
        (2, "DEACTIVATE"),
        (3, "REQUERY"),
        (4, "ON_MOVE"),
        (5, "GET_EXTRAS"),
        (6, "RESPOND"),
        (7, "CLOSE"),
    ];

    fn handle(&self, _name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        ap::no_exception(reply)?;
        reply.write_i32(0)?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub enum CellValue {
    Null,
    Integer(i64),
    #[allow(dead_code)]
    Float(f64),
    String(String),
}

pub struct CursorWindowBuilder {
    name: String,
    column_names: Vec<String>,
    rows: Vec<Vec<CellValue>>,
}

impl CursorWindowBuilder {
    pub fn new(name: impl Into<String>, column_names: Vec<String>) -> Self {
        Self {
            name: name.into(),
            column_names,
            rows: Vec::new(),
        }
    }

    pub fn add_row(&mut self, row: Vec<CellValue>) {
        self.rows.push(row);
    }

    pub fn write_to_parcel(self, p: &mut Parcel) -> Result<()> {
        let num_rows = self.rows.len();
        let num_cols = self.column_names.len();
        let slots_size = num_rows * num_cols * 16;

        let mut heap = Vec::new();
        let mut add_string = |s: &str| -> (u32, u32) {
            let offset = heap.len() as u32;
            let bytes = s.as_bytes();
            heap.extend_from_slice(bytes);
            heap.push(0);
            let size = (bytes.len() + 1) as u32;
            while heap.len() % 4 != 0 {
                heap.push(0);
            }
            (offset, size)
        };

        let mut slots = vec![0u8; slots_size];
        for (r, row) in self.rows.iter().enumerate() {
            for c in 0..num_cols {
                let val = row.get(c).unwrap_or(&CellValue::Null);
                let slot_idx = r * num_cols + c;
                let slot_offset = slots_size - ((slot_idx + 1) * 16);
                let slot = &mut slots[slot_offset..slot_offset + 16];

                match val {
                    CellValue::Null => {
                        slot[0..4].copy_from_slice(&0i32.to_le_bytes()); // FIELD_TYPE_NULL = 0
                    }
                    CellValue::Integer(v) => {
                        slot[0..4].copy_from_slice(&1i32.to_le_bytes()); // FIELD_TYPE_INTEGER = 1
                        slot[4..12].copy_from_slice(&v.to_le_bytes());
                    }
                    CellValue::Float(v) => {
                        slot[0..4].copy_from_slice(&2i32.to_le_bytes()); // FIELD_TYPE_FLOAT = 2
                        slot[4..12].copy_from_slice(&v.to_le_bytes());
                    }
                    CellValue::String(s) => {
                        let (str_offset, str_size) = add_string(s);
                        slot[0..4].copy_from_slice(&3i32.to_le_bytes()); // FIELD_TYPE_STRING = 3
                        slot[4..8].copy_from_slice(&str_offset.to_le_bytes());
                        slot[8..12].copy_from_slice(&str_size.to_le_bytes());
                    }
                }
            }
        }

        let alloc_offset = heap.len();
        let compacted_size = alloc_offset + slots_size;

        p.write_i32(0)?; // mStartPos = 0
        ap::string8(p, Some(&self.name))?;
        p.write_u32(num_rows as u32)?;
        p.write_u32(num_cols as u32)?;

        let mut dest = heap;
        dest.extend_from_slice(&slots);
        while dest.len() % 4 != 0 {
            dest.push(0);
        }

        if compacted_size <= 16384 {
            p.write_u32(compacted_size as u32)?;
            p.write_i32(0)?; // isAshmem = false
            for chunk in dest.chunks_exact(4) {
                p.write_u32(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))?;
            }
        } else {
            use std::io::Write;
            use std::os::fd::{AsFd, FromRawFd};
            let cname = std::ffi::CString::new("CursorWindow").unwrap();
            let raw_fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
            if raw_fd < 0 {
                return Err(rsbinder::StatusCode::FailedTransaction.into());
            }
            let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
            file.set_len(compacted_size as u64)
                .map_err(|_| rsbinder::StatusCode::FailedTransaction)?;
            file.write_all(&dest)
                .map_err(|_| rsbinder::StatusCode::FailedTransaction)?;

            p.write_u32(compacted_size as u32)?;
            p.write_i32(1)?; // isAshmem = true
            p.write_raw_file_descriptor(file.as_fd())?;
        }

        Ok(())
    }
}

pub fn write_query_reply(
    reply: &mut Parcel,
    bulk_cursor: &SIBinder,
    column_names: &[String],
    row_count: usize,
    window: CursorWindowBuilder,
) -> Result<bool> {
    ap::no_exception(reply)?;
    reply.write_i32(1)?; // has BulkCursorDescriptor
    reply.write(&Some(bulk_cursor.clone()))?;
    // columnNames: String[]
    reply.write_i32(column_names.len() as i32)?;
    for col in column_names {
        ap::string16(reply, Some(col))?;
    }
    reply.write_i32(0)?; // wantsAllOnMoveCalls = false
    reply.write_i32(row_count as i32)?; // count
    reply.write_i32(1)?; // has window
    window.write_to_parcel(reply)?;
    Ok(true)
}
