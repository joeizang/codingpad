use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

const MAX_VECTOR_ELEMENTS: usize = 1024 * 1024;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_KEY_BYTES: usize = 96;
static WRITES: Mutex<()> = Mutex::new(());
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Reuse a cached value for this key on later runs of the same script.
/// Outside CodingPad, evaluate the factory directly.
///
/// Entries are limited to 1 MiB and keys to 96 UTF-8 bytes. The cache keeps
/// the latest 32 stored keys per script.
///
/// # Panics
///
/// Panics if the key or encoded value exceeds these limits, if the value
/// fails to encode, or if the cache directory cannot be read or written.
pub fn cache<T: CacheValue>(key: &str, factory: impl FnOnce() -> T) -> T {
    let Some(root) = std::env::var_os("CODINGPAD_RUST_CACHE") else {
        return factory();
    };
    assert!(
        key.len() <= MAX_KEY_BYTES,
        "cache() key is {} UTF-8 bytes, exceeding the {MAX_KEY_BYTES}-byte limit",
        key.len()
    );
    let entry = PathBuf::from(root).join(format!(
        "{}.cache",
        key.as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let read = || {
        read_entry(&entry).unwrap_or_else(|error| panic!("cache() could not read {key:?}: {error}"))
    };
    if let Some(value) = read() {
        return value;
    }

    let value = factory();
    // Let the factory call cache() and let unrelated reads continue.
    let _writes = WRITES.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(value) = read() {
        return value;
    }
    store_entry(&entry, &value)
        .unwrap_or_else(|error| panic!("cache() could not store {key:?}: {error}"));
    value
}

/// A value that can be written to and read from CodingPad's disk cache.
/// Implement this trait to cache a custom type.
pub trait CacheValue: Sized {
    fn encode(&self, output: &mut dyn Write) -> io::Result<()>;
    fn decode(input: &mut &[u8]) -> io::Result<Self>;
}

macro_rules! number {
    ($($ty:ty),*) => {$(
        impl CacheValue for $ty {
            fn encode(&self, output: &mut dyn Write) -> io::Result<()> {
                output.write_all(&self.to_le_bytes())
            }
            fn decode(input: &mut &[u8]) -> io::Result<Self> {
                let mut bytes = [0; std::mem::size_of::<Self>()];
                input.read_exact(&mut bytes)?;
                Ok(Self::from_le_bytes(bytes))
            }
        }
    )*};
}
number!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);

impl CacheValue for bool {
    fn encode(&self, output: &mut dyn Write) -> io::Result<()> {
        u8::from(*self).encode(output)
    }

    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        match u8::decode(input)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid boolean",
            )),
        }
    }
}

impl CacheValue for String {
    fn encode(&self, output: &mut dyn Write) -> io::Result<()> {
        encode_length(self.len(), output)?;
        output.write_all(self.as_bytes())
    }

    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        let len = u32::decode(input)? as usize;
        if len > input.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let (text, rest) = input.split_at(len);
        *input = rest;
        String::from_utf8(text.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl<T: CacheValue> CacheValue for Vec<T> {
    fn encode(&self, output: &mut dyn Write) -> io::Result<()> {
        if self.len() > MAX_VECTOR_ELEMENTS {
            return Err(too_many_elements());
        }
        encode_length(self.len(), output)?;
        self.iter().try_for_each(|value| value.encode(output))
    }

    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        let len = u32::decode(input)? as usize;
        if len > MAX_VECTOR_ELEMENTS {
            return Err(too_many_elements());
        }
        (0..len).map(|_| T::decode(input)).collect()
    }
}

impl<T: CacheValue> CacheValue for Option<T> {
    fn encode(&self, output: &mut dyn Write) -> io::Result<()> {
        self.is_some().encode(output)?;
        if let Some(value) = self {
            value.encode(output)?;
        }
        Ok(())
    }

    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        bool::decode(input)?.then(|| T::decode(input)).transpose()
    }
}

fn encode_length(len: usize, output: &mut dyn Write) -> io::Result<()> {
    if len > MAX_BYTES {
        return Err(too_large());
    }
    (len as u32).encode(output)
}

fn read_entry<T: CacheValue>(entry: &Path) -> io::Result<Option<T>> {
    let file = match File::open(entry) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Ok(None);
    }
    let mut input = bytes.as_slice();
    if String::decode(&mut input).ok().as_deref() != Some(std::any::type_name::<T>()) {
        return Ok(None);
    }
    Ok(T::decode(&mut input).ok().filter(|_| input.is_empty()))
}

fn store_entry(entry: &Path, value: &impl CacheValue) -> io::Result<()> {
    struct Entry(Vec<u8>);
    impl Write for Entry {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_BYTES - self.0.len() {
                return Err(too_large());
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut data = Entry(Vec::new());
    std::any::type_name_of_val(value)
        .to_owned()
        .encode(&mut data)?;
    value.encode(&mut data)?;
    let root = entry.parent().unwrap();
    fs::create_dir_all(root)?;
    let mut entries = Vec::new();
    for candidate in fs::read_dir(root)? {
        let candidate = candidate?;
        if candidate.file_type()?.is_file()
            && candidate
                .path()
                .extension()
                .is_some_and(|ext| ext == "cache")
            && candidate.path() != entry
        {
            entries.push((candidate.metadata()?.modified()?, candidate.path()));
        }
    }
    if entries.len() >= MAX_ENTRIES {
        entries.sort_unstable();
        let remove = entries.len() + 1 - MAX_ENTRIES;
        for (_, candidate) in entries.into_iter().take(remove) {
            fs::remove_file(candidate)?;
        }
    }
    let temp = root.join(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&data.0)?;
        drop(file);
        fs::rename(&temp, entry)
    };
    let result = write();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("a cache value must fit in {MAX_BYTES} bytes"),
    )
}

fn too_many_elements() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("a cached vector must fit in {MAX_VECTOR_ELEMENTS} elements"),
    )
}
