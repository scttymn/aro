//! Standard gralloc metadata, encoded the way libui's gralloc4 decoders
//! expect: every blob starts with the encoded MetadataType (string name,
//! int64 value), then the value. Integers are raw little-endian; strings are
//! int64 length + bytes; ExtendableType is string + int64.
use crate::handle::Layout;
use crate::AIMapper_MetadataTypeDescription;

pub const STANDARD_TYPE: &str = "android.hardware.graphics.common.StandardMetadataType";

pub const BUFFER_ID: i64 = 1;
pub const NAME: i64 = 2;
pub const WIDTH: i64 = 3;
pub const HEIGHT: i64 = 4;
pub const LAYER_COUNT: i64 = 5;
pub const PIXEL_FORMAT_REQUESTED: i64 = 6;
pub const PIXEL_FORMAT_FOURCC: i64 = 7;
pub const PIXEL_FORMAT_MODIFIER: i64 = 8;
pub const USAGE: i64 = 9;
pub const ALLOCATION_SIZE: i64 = 10;
pub const PROTECTED_CONTENT: i64 = 11;
pub const COMPRESSION: i64 = 12;
pub const INTERLACED: i64 = 13;
pub const CHROMA_SITING: i64 = 14;
pub const PLANE_LAYOUTS: i64 = 15;
pub const CROP: i64 = 16;
pub const DATASPACE: i64 = 17;
pub const BLEND_MODE: i64 = 18;
pub const SMPTE2086: i64 = 19;
pub const CTA861_3: i64 = 20;
pub const SMPTE2094_40: i64 = 21;
pub const SMPTE2094_10: i64 = 22;
pub const STRIDE: i64 = 23;

// Android PixelFormat values we allocate.
pub const RGBA_8888: i32 = 1;
pub const RGBX_8888: i32 = 2;
pub const RGB_888: i32 = 3;
pub const RGB_565: i32 = 4;
pub const BGRA_8888: i32 = 5;

// DRM fourcc codes for the formats above (little-endian byte order names).
fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}

pub fn bytes_per_pixel(format: i32) -> Option<u32> {
    match format {
        RGBA_8888 | RGBX_8888 | BGRA_8888 => Some(4),
        RGB_888 => Some(3),
        RGB_565 => Some(2),
        _ => None,
    }
}

struct Enc(Vec<u8>);
impl Enc {
    fn i32(&mut self, v: i32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn i64(&mut self, v: i64) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u64(&mut self, v: u64) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn string(&mut self, s: &str) {
        self.i64(s.len() as i64);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn extendable(&mut self, name: &str, value: i64) {
        self.string(name);
        self.i64(value);
    }
}

const PLANE_LAYOUT_COMPONENT_TYPE: &str = "android.hardware.graphics.common.PlaneLayoutComponentType";

/// Encode one standard metadata value for `layout`, or None if unsupported.
pub fn encode(l: &Layout, ty: i64) -> Option<Vec<u8>> {
    let mut e = Enc(Vec::with_capacity(96));
    e.extendable(STANDARD_TYPE, ty);
    let bpp = bytes_per_pixel(l.format).unwrap_or(4);
    match ty {
        BUFFER_ID => e.u64(l.id),
        NAME => e.string("aro"),
        WIDTH => e.u64(l.width as u64),
        HEIGHT => e.u64(l.height as u64),
        LAYER_COUNT => e.u64(l.layer_count as u64),
        PIXEL_FORMAT_REQUESTED => e.i32(l.format),
        PIXEL_FORMAT_FOURCC => e.u32(match l.format {
            RGBA_8888 => fourcc(b'A', b'B', b'2', b'4'),
            RGBX_8888 => fourcc(b'X', b'B', b'2', b'4'),
            BGRA_8888 => fourcc(b'A', b'R', b'2', b'4'),
            RGB_888 => fourcc(b'B', b'G', b'2', b'4'),
            RGB_565 => fourcc(b'R', b'G', b'1', b'6'),
            _ => 0,
        }),
        PIXEL_FORMAT_MODIFIER => e.u64(0), // DRM_FORMAT_MOD_LINEAR
        USAGE => e.u64(l.usage),
        ALLOCATION_SIZE => e.u64(l.size as u64),
        PROTECTED_CONTENT => e.u64(0),
        COMPRESSION => e.extendable("android.hardware.graphics.common.Compression", 0),
        INTERLACED => e.extendable("android.hardware.graphics.common.Interlaced", 0),
        CHROMA_SITING => e.extendable("android.hardware.graphics.common.ChromaSiting", 0),
        PLANE_LAYOUTS => {
            // One interleaved plane.
            let comps: &[(i64, i64)] = match l.format {
                RGBA_8888 => &[(1, 0), (2, 8), (4, 16), (8, 24)],
                RGBX_8888 => &[(1, 0), (2, 8), (4, 16)],
                BGRA_8888 => &[(4, 0), (2, 8), (1, 16), (8, 24)],
                RGB_888 => &[(1, 0), (2, 8), (4, 16)],
                RGB_565 => &[(1, 0), (2, 5), (4, 11)],
                _ => &[],
            };
            e.i64(1); // number of planes
            e.i64(comps.len() as i64);
            for &(kind, offset) in comps {
                e.extendable(PLANE_LAYOUT_COMPONENT_TYPE, kind);
                e.i64(offset); // offsetInBits
                e.i64(if l.format == RGB_565 { if kind == 2 { 6 } else { 5 } } else { 8 }); // sizeInBits
            }
            e.i64(0); // offsetInBytes
            e.i64((bpp * 8) as i64); // sampleIncrementInBits
            e.i64((l.stride * bpp) as i64); // strideInBytes
            e.i64(l.width as i64); // widthInSamples
            e.i64(l.height as i64); // heightInSamples
            e.i64((l.stride * bpp * l.height) as i64); // totalSizeInBytes
            e.i64(1); // horizontalSubsampling
            e.i64(1); // verticalSubsampling
        }
        CROP => {
            e.i64(1); // one rect (per plane)
            e.i32(0);
            e.i32(0);
            e.i32(l.width as i32);
            e.i32(l.height as i32);
        }
        DATASPACE => e.i32(0), // UNKNOWN
        BLEND_MODE => e.i32(0), // INVALID
        SMPTE2086 | CTA861_3 | SMPTE2094_40 | SMPTE2094_10 => {} // optional: absent
        STRIDE => e.u64(l.stride as u64),
        _ => return None,
    }
    Some(e.0)
}

macro_rules! desc {
    ($v:expr, $d:expr, $set:expr) => {
        AIMapper_MetadataTypeDescription {
            metadata_type: crate::AIMapper_MetadataType { name: concat!("android.hardware.graphics.common.StandardMetadataType", "\0").as_ptr() as *const _, value: $v },
            description: concat!($d, "\0").as_ptr() as *const _,
            is_gettable: true,
            is_settable: $set,
            reserved: [0; 32],
        }
    };
}

pub static DESCRIPTIONS: [AIMapper_MetadataTypeDescription; 23] = [
    desc!(BUFFER_ID, "buffer id", false),
    desc!(NAME, "name", false),
    desc!(WIDTH, "width", false),
    desc!(HEIGHT, "height", false),
    desc!(LAYER_COUNT, "layer count", false),
    desc!(PIXEL_FORMAT_REQUESTED, "pixel format requested", false),
    desc!(PIXEL_FORMAT_FOURCC, "fourcc", false),
    desc!(PIXEL_FORMAT_MODIFIER, "modifier", false),
    desc!(USAGE, "usage", false),
    desc!(ALLOCATION_SIZE, "allocation size", false),
    desc!(PROTECTED_CONTENT, "protected content", false),
    desc!(COMPRESSION, "compression", false),
    desc!(INTERLACED, "interlaced", false),
    desc!(CHROMA_SITING, "chroma siting", false),
    desc!(PLANE_LAYOUTS, "plane layouts", false),
    desc!(CROP, "crop", true),
    desc!(DATASPACE, "dataspace", true),
    desc!(BLEND_MODE, "blend mode", true),
    desc!(SMPTE2086, "smpte2086", true),
    desc!(CTA861_3, "cta861.3", true),
    desc!(SMPTE2094_40, "smpte2094-40", true),
    desc!(SMPTE2094_10, "smpte2094-10", true),
    desc!(STRIDE, "stride", false),
];
unsafe impl Sync for AIMapper_MetadataTypeDescription {}
