use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use sha2::Digest;

pub trait HashingReader {
    fn hash(&self) -> String;
}

/// This struct wraps a reader and computes the SHA256 hash of the data as it is read.
///
/// SHA256 hashes are used in most images except OS images.
pub struct HashingReader256<R: Read>(R, sha2::Sha256);
impl<R: Read> HashingReader256<R> {
    #[allow(dead_code)]
    pub fn new(reader: R) -> Self {
        Self(reader, sha2::Sha256::new())
    }
}
impl<R: Read> HashingReader for HashingReader256<R> {
    fn hash(&self) -> String {
        format!("{:x}", self.1.clone().finalize())
    }
}
impl<R: Read> Read for HashingReader256<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Read the requested amount of data from the inner reader
        let n = self.0.read(buf)?;
        // Update the hash with the data we read
        self.1.update(&buf[..n]);
        // Return the number of bytes read
        Ok(n)
    }
}

/// This struct wraps a reader and computes the SHA384 hash of the data as it is read.
///
/// SHA384 hashes are primarily used for OS images.
pub struct HashingReader384<R: Read>(R, sha2::Sha384);
impl<R: Read> HashingReader384<R> {
    pub fn new(reader: R) -> Self {
        Self(reader, sha2::Sha384::new())
    }
}
impl<R: Read> HashingReader for HashingReader384<R> {
    fn hash(&self) -> String {
        format!("{:x}", self.1.clone().finalize())
    }
}
impl<R: Read> Read for HashingReader384<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Read the requested amount of data from the inner reader
        let n = self.0.read(buf)?;
        // Update the hash with the data we read
        self.1.update(&buf[..n]);
        // Return the number of bytes read
        Ok(n)
    }
}

pub fn compute_file_hash(path: &Path) -> io::Result<(u64, String)> {
    let mut bytes_read = 0;
    let mut reader = File::open(path)?;
    let mut hasher = sha2::Sha384::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes_read += n as u64;
        hasher.update(&buf[..n]);
    }

    Ok((bytes_read, format!("{:x}", hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    #[test]
    fn test_hashing_reader_256() {
        let input = b"Hello, world!";
        let mut hasher = HashingReader256::new(Cursor::new(&input));

        let mut output = Vec::new();
        hasher.read_to_end(&mut output).unwrap();
        assert_eq!(input, &*output);
        assert_eq!(
            hasher.hash(),
            "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
        );
    }

    #[test]
    fn test_hashing_reader_384() {
        let input = b"Hello, world!";
        let mut hasher = HashingReader384::new(Cursor::new(&input));

        let mut output = Vec::new();
        hasher.read_to_end(&mut output).unwrap();
        assert_eq!(input, &*output);
        assert_eq!(
            hasher.hash(),
            "55bc556b0d2fe0fce582ba5fe07baafff035653638c7ac0d5494c2a64c0bea1cc57331c7c12a45cdbca7f4c34a089eeb"
        );
    }
}
