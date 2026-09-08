# third_party

- `rsbinder/` — rsbinder 0.10.0 (Apache-2.0, https://github.com/hiking90/rsbinder), vendored
  via `[patch.crates-io]` with one addition: `Parcel::write_raw_file_descriptor`, needed to
  send a bare fd object the way Java's `writeRawFileDescriptor` does. To be proposed upstream.
