use anyhow::{Result, bail};
use flate2::read::{ZlibDecoder, ZlibEncoder};
use flate2::Compression as ZlibCompression;
use std::io::Read;
use zstd;

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Zstd,
    Zlib,
    None,
}

impl Algorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Algorithm::Zstd => "zstd",
            Algorithm::Zlib => "zlib",
            Algorithm::None => "none",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "zstd" => Ok(Algorithm::Zstd),
            "zlib" => Ok(Algorithm::Zlib),
            "none" => Ok(Algorithm::None),
            _ => bail!("unknown compression algorithm: {}", s),
        }
    }
}

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compression configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub algorithm: Algorithm,
    /// Compression level. For zstd: 1-19 (default 3). For zlib: 0-9 (default 6).
    pub level: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::Zstd,
            level: 3,
        }
    }
}

/// Compress data using the configured algorithm.
pub fn compress(data: &[u8], config: &Config) -> Result<Vec<u8>> {
    match config.algorithm {
        Algorithm::Zstd => {
            let compressed = zstd::encode_all(data, config.level)?;
            Ok(compressed)
        }
        Algorithm::Zlib => {
            let mut encoder = ZlibEncoder::new(data, ZlibCompression::new(config.level as u32));
            let mut compressed = Vec::new();
            encoder.read_to_end(&mut compressed)?;
            Ok(compressed)
        }
        Algorithm::None => Ok(data.to_vec()),
    }
}

/// Decompress data using the specified algorithm.
pub fn decompress(data: &[u8], algorithm: &str) -> Result<Vec<u8>> {
    match algorithm {
        "zstd" => {
            let decompressed = zstd::decode_all(data)?;
            Ok(decompressed)
        }
        "zlib" => {
            let mut decoder = ZlibDecoder::new(data);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            Ok(decompressed)
        }
        "none" => Ok(data.to_vec()),
        _ => bail!("unknown compression algorithm: {}", algorithm),
    }
}

/// Returns the compression ratio (compressed / original). Lower is better.
pub fn ratio(original_size: usize, compressed_size: usize) -> f64 {
    if original_size == 0 {
        return 1.0;
    }
    compressed_size as f64 / original_size as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zstd_roundtrip() {
        let data = b"Hello, SQLite-FS! This is a test of zstd compression.".repeat(100);
        let config = Config::default();
        let compressed = compress(&data, &config).unwrap();
        assert!(compressed.len() < data.len());
        let decompressed = decompress(&compressed, "zstd").unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_zlib_roundtrip() {
        let data = b"Hello, SQLite-FS! This is a test of zlib compression.".repeat(100);
        let config = Config {
            algorithm: Algorithm::Zlib,
            level: 6,
        };
        let compressed = compress(&data, &config).unwrap();
        assert!(compressed.len() < data.len());
        let decompressed = decompress(&compressed, "zlib").unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_none_roundtrip() {
        let data = b"No compression test data";
        let config = Config {
            algorithm: Algorithm::None,
            level: 0,
        };
        let compressed = compress(data, &config).unwrap();
        assert_eq!(compressed, data);
        let decompressed = decompress(&compressed, "none").unwrap();
        assert_eq!(decompressed, data.to_vec());
    }

    #[test]
    fn test_algorithm_from_str() {
        assert_eq!(Algorithm::from_str("zstd").unwrap(), Algorithm::Zstd);
        assert_eq!(Algorithm::from_str("zlib").unwrap(), Algorithm::Zlib);
        assert_eq!(Algorithm::from_str("none").unwrap(), Algorithm::None);
        assert!(Algorithm::from_str("invalid").is_err());
    }

    #[test]
    fn test_ratio() {
        assert!((ratio(1000, 500) - 0.5).abs() < f64::EPSILON);
        assert!((ratio(0, 0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_zstd_levels() {
        let data = b"Compression level test data. ".repeat(1000);
        let fast = compress(&data, &Config { algorithm: Algorithm::Zstd, level: 1 }).unwrap();
        let high = compress(&data, &Config { algorithm: Algorithm::Zstd, level: 19 }).unwrap();
        // Higher levels should generally produce smaller output
        assert!(high.len() <= fast.len());
        // Both should decompress correctly
        assert_eq!(decompress(&fast, "zstd").unwrap(), data.to_vec());
        assert_eq!(decompress(&high, "zstd").unwrap(), data.to_vec());
    }
}
