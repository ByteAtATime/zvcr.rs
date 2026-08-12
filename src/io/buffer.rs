use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::Arc;

thread_local! {
    static POOL: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

const MIN_POOL_CAPACITY: usize = 1 << 20;
const MAX_POOL_DEPTH: usize = 4;

struct PoolEntry {
    buf: ManuallyDrop<Vec<u8>>,
}

impl Drop for PoolEntry {
    fn drop(&mut self) {
        let mut buf = unsafe { ManuallyDrop::take(&mut self.buf) };
        if buf.capacity() >= MIN_POOL_CAPACITY {
            buf.clear();
            POOL.with(|p| {
                if p.borrow().len() < MAX_POOL_DEPTH {
                    p.borrow_mut().push(buf);
                }
            });
        }
    }
}

pub struct PooledBytes {
    entry: Arc<PoolEntry>,
    start: usize,
    len: usize,
}

impl PooledBytes {
    pub fn from_vec(vec: Vec<u8>) -> Self {
        let len = vec.len();
        Self {
            entry: Arc::new(PoolEntry {
                buf: ManuallyDrop::new(vec),
            }),
            start: 0,
            len,
        }
    }

    pub fn slice(&self, start: usize, len: usize) -> Self {
        Self {
            entry: Arc::clone(&self.entry),
            start: self.start + start,
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Deref for PooledBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        let full: &Vec<u8> = &self.entry.buf;
        &full[self.start..self.start + self.len]
    }
}

impl Clone for PooledBytes {
    fn clone(&self) -> Self {
        Self {
            entry: Arc::clone(&self.entry),
            start: self.start,
            len: self.len,
        }
    }
}

impl PartialEq for PooledBytes {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for PooledBytes {}

impl std::fmt::Debug for PooledBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

pub fn take_pooled_buffer(min_cap: usize) -> Vec<u8> {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        for i in 0..pool.len() {
            if pool[i].capacity() >= min_cap {
                return pool.swap_remove(i);
            }
        }
        Vec::with_capacity(min_cap)
    })
}
