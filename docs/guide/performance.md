[← The manual](README.md)

# Performance: 4K and many sources

## Performance: 4K and many sources

- **Encoders:** every recorded source, the program, the proxies and the stream
  is one encode running at once, all on the media engine. Camera frames go to
  it without being copied.
  - The M-series Pro, Max and Ultra chips have more media engines than the
    base chips, and encode ProRes in hardware.
  - For lots of 4K, use H.265 or ProRes and moderate bitrates.
  - If an encoder can't keep up, the status light (Now and the Log) says so, and
    frames are repeated in that file rather than anything stalling.
  - With the encoder on **Automatic**, a file or stream whose encoder won't
    start (on Windows a graphics card can run out of encoder sessions) moves
    to the next encoder on the list instead of failing, and the Log says so.
- **Compositor:** runs on the GPU (Metal). A 4K program is fine.
- **USB bandwidth:** several 4K USB capture devices on one bus can run out of
  bandwidth. Spread them across ports, or pick an MJPEG mode in the source's
  settings (Cutaway decodes it automatically).
