use crate::io::buffer::PooledBytes;
use crate::io::serialize::error::ReadError;

pub(crate) fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub(crate) fn put_u16_le(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32_le(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u64_le(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_bytes(buf: &mut Vec<u8>, v: &[u8]) {
    buf.extend_from_slice(v);
}

#[cfg(not(target_endian = "little"))]
compile_error!("only little-endian targets are supported");

pub(crate) struct ByteCursor {
    pub(crate) data: PooledBytes,
    pub(crate) pos: usize,
}

impl ByteCursor {
    pub(crate) fn new(data: PooledBytes) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub(crate) fn take_slice(&mut self, n: usize) -> Result<PooledBytes, ReadError> {
        if self.pos + n > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        let slice = self.data.slice(self.pos, n);
        self.pos += n;
        Ok(slice)
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: usize) -> Result<(), ReadError> {
        if self.pos + n > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        self.pos += n;
        Ok(())
    }

    #[inline]
    pub(crate) fn read_bytes<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        if self.pos + N > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + N]);
        self.pos += N;
        Ok(bytes)
    }

    #[inline]
    pub(crate) fn read_u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.read_bytes::<1>()?[0])
    }

    #[inline]
    pub(crate) fn read_u16(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_le_bytes(self.read_bytes::<2>()?))
    }

    #[inline]
    pub(crate) fn read_u32(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_le_bytes(self.read_bytes::<4>()?))
    }

    #[inline]
    pub(crate) fn read_u64(&mut self) -> Result<u64, ReadError> {
        Ok(u64::from_le_bytes(self.read_bytes::<8>()?))
    }

    #[inline]
    pub(crate) fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ReadError> {
        let n = buf.len();
        if self.pos + n > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        buf.copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(())
    }
}
