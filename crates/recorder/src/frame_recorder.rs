//! Raw WebSocket frame recording.
//!
//! Records raw frames for perfect replay capability.

use crate::RecorderError;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Raw WebSocket frame with receive timestamp.
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// Monotonic receive timestamp (nanoseconds).
    pub recv_mono_ns: u64,
    /// Wall clock timestamp (milliseconds since epoch).
    pub wall_time_ms: u64,
    /// Raw frame bytes.
    pub data: Vec<u8>,
}

impl RawFrame {
    pub fn new(recv_mono_ns: u64, wall_time_ms: u64, data: Vec<u8>) -> Self {
        Self {
            recv_mono_ns,
            wall_time_ms,
            data,
        }
    }

    /// Serialize frame to bytes (length-prefixed format).
    /// Format: [recv_mono_ns: u64][wall_time_ms: u64][len: u32][data: bytes]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(20 + self.data.len());
        buf.extend_from_slice(&self.recv_mono_ns.to_le_bytes());
        buf.extend_from_slice(&self.wall_time_ms.to_le_bytes());
        buf.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Deserialize frame from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<(Self, usize)> {
        if bytes.len() < 20 {
            return None;
        }

        let recv_mono_ns = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let wall_time_ms = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let len = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;

        if bytes.len() < 20 + len {
            return None;
        }

        let data = bytes[20..20 + len].to_vec();
        let frame = Self {
            recv_mono_ns,
            wall_time_ms,
            data,
        };

        Some((frame, 20 + len))
    }
}

/// Records raw WebSocket frames to gzipped file.
pub struct FrameRecorder {
    writer: Option<GzEncoder<BufWriter<File>>>,
    frames_written: u64,
    bytes_written: u64,
}

impl FrameRecorder {
    pub fn new() -> Self {
        Self {
            writer: None,
            frames_written: 0,
            bytes_written: 0,
        }
    }

    /// Open a new recording file.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), RecorderError> {
        let file = File::create(path)?;
        let buf_writer = BufWriter::with_capacity(64 * 1024, file);
        self.writer = Some(GzEncoder::new(buf_writer, Compression::fast()));
        self.frames_written = 0;
        self.bytes_written = 0;
        Ok(())
    }

    /// Record a raw frame.
    pub fn record(&mut self, frame: &RawFrame) -> Result<(), RecorderError> {
        let writer = self.writer.as_mut().ok_or(RecorderError::NotOpen)?;
        let bytes = frame.to_bytes();
        writer.write_all(&bytes)?;
        self.frames_written += 1;
        self.bytes_written += bytes.len() as u64;
        Ok(())
    }

    /// Record raw bytes with timestamp.
    pub fn record_raw(
        &mut self,
        recv_mono_ns: u64,
        wall_time_ms: u64,
        data: &[u8],
    ) -> Result<(), RecorderError> {
        let frame = RawFrame {
            recv_mono_ns,
            wall_time_ms,
            data: data.to_vec(),
        };
        self.record(&frame)
    }

    /// Flush buffered data.
    pub fn flush(&mut self) -> Result<(), RecorderError> {
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        Ok(())
    }

    /// Close the recording file.
    pub fn close(&mut self) -> Result<(), RecorderError> {
        if let Some(writer) = self.writer.take() {
            writer.finish()?;
        }
        Ok(())
    }

    /// Get number of frames written.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Get number of bytes written (before compression).
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Check if recorder is open.
    pub fn is_open(&self) -> bool {
        self.writer.is_some()
    }
}

impl Default for FrameRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FrameRecorder {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Reader for recorded frames.
pub struct FrameReader {
    data: Vec<u8>,
    offset: usize,
}

impl FrameReader {
    /// Load recorded frames from gzipped file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RecorderError> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let file = File::open(path)?;
        let mut decoder = GzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;

        Ok(Self { data, offset: 0 })
    }

    /// Read the next frame.
    pub fn next_frame(&mut self) -> Option<RawFrame> {
        if self.offset >= self.data.len() {
            return None;
        }

        let (frame, consumed) = RawFrame::from_bytes(&self.data[self.offset..])?;
        self.offset += consumed;
        Some(frame)
    }

    /// Reset to beginning.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Check if all frames have been read.
    pub fn is_exhausted(&self) -> bool {
        self.offset >= self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_frame_serialization() {
        let frame = RawFrame::new(123456789, 1000, b"hello world".to_vec());

        let bytes = frame.to_bytes();
        let (decoded, consumed) = RawFrame::from_bytes(&bytes).unwrap();

        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded.recv_mono_ns, 123456789);
        assert_eq!(decoded.wall_time_ms, 1000);
        assert_eq!(decoded.data, b"hello world");
    }

    #[test]
    fn test_frame_recorder() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("frames.bin.gz");

        let mut recorder = FrameRecorder::new();
        recorder.open(&path).unwrap();

        // Write some frames
        for i in 0..100 {
            recorder
                .record_raw(i * 1000, i * 100, format!("frame-{}", i).as_bytes())
                .unwrap();
        }

        assert_eq!(recorder.frames_written(), 100);
        recorder.close().unwrap();

        // Read them back
        let mut reader = FrameReader::open(&path).unwrap();
        let mut count = 0;
        while let Some(frame) = reader.next_frame() {
            assert_eq!(frame.recv_mono_ns, count * 1000);
            count += 1;
        }
        assert_eq!(count, 100);
    }
}
