use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use sha2::Digest;

/// This struct wraps a reader and computes the SHA256 hash of the data as it is read.
pub struct HashingReader<R: Read>(R, sha2::Sha256);
impl<R: Read> HashingReader<R> {
    pub fn new(reader: R) -> Self {
        Self(reader, sha2::Sha256::new())
    }

    pub fn hash(&self) -> String {
        format!("{:x}", self.1.clone().finalize())
    }
}
impl<R: Read> Read for HashingReader<R> {
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
    fn test_hashing_reader() {
        let input = b"Hello, world!";
        let mut hasher = HashingReader::new(Cursor::new(&input));

        let mut output = Vec::new();
        hasher.read_to_end(&mut output).unwrap();
        assert_eq!(input, &*output);
        assert_eq!(
            hasher.hash(),
            "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
        );
    }
}
