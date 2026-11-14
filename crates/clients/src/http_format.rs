//! Allocation-free request formatting into disjoint native-owned storage.
//! Byte/C-string behavior and snprintf's clamped length/terminator are explicit.
use std::mem::MaybeUninit;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClientsFormatSpan {
    pub data: *const u8,
    pub length: usize,
}
#[repr(C)]
pub struct ClientsFormatResult {
    pub written: usize,
    ///0 success;1 invalid span/output;2 printf count overflow (native errno).
    pub error: u32,
}
/// Where a request is formatted: native storage, which may be uninitialized, or a
/// Rust buffer.
pub trait Output {
    fn capacity(&self) -> usize;
    /// Writes `bytes` at `offset`, which the writer keeps inside the capacity.
    fn put(&mut self, offset: usize, bytes: &[u8]);
}
impl Output for [MaybeUninit<u8>] {
    fn capacity(&self) -> usize {
        self.len()
    }
    fn put(&mut self, offset: usize, bytes: &[u8]) {
        if let Some(to) = self.get_mut(offset..offset + bytes.len()) {
            to.write_copy_of_slice(bytes);
        }
    }
}
impl Output for [u8] {
    fn capacity(&self) -> usize {
        self.len()
    }
    fn put(&mut self, offset: usize, bytes: &[u8]) {
        if let Some(to) = self.get_mut(offset..offset + bytes.len()) {
            to.copy_from_slice(bytes);
        }
    }
}
struct Writer<'a, O: Output + ?Sized> {
    output: &'a mut O,
    capacity: usize,
    count: usize,
}
impl<O: Output + ?Sized> Writer<'_, O> {
    fn append(&mut self, bytes: &[u8]) -> Option<()> {
        let count = self.count.checked_add(bytes.len())?;
        let offset = self.count.min(self.capacity.saturating_sub(1));
        let copied = bytes
            .len()
            .min(self.capacity.saturating_sub(1).saturating_sub(offset));
        // `offset + copied <= capacity - 1`.
        self.output
            .put(offset, bytes.get(..copied).unwrap_or_default());
        self.count = count;
        Some(())
    }
    fn decimal(&mut self, number: usize) -> Option<()> {
        let mut digits = [0_u8; usize::BITS as usize];
        self.append(crate::format_into(&mut digits, format_args!("{number}"))?)
    }
    fn finish(self, error: u32) -> ClientsFormatResult {
        let overflow = error == 2 || self.count > i32::MAX as usize;
        let error = if overflow { 2 } else { error };
        let written = if error != 0 {
            if overflow && self.capacity == 0 {
                usize::MAX
            } else {
                0
            }
        } else if self.capacity == 0 {
            self.count
        } else {
            self.count.min(self.capacity - 1)
        };
        if self.capacity != 0 {
            // `written <= capacity - 1` (or 0), inside the output.
            self.output.put(if error == 0 { written } else { 0 }, &[0]);
        }
        ClientsFormatResult { written, error }
    }
}
fn precision_string(bytes: &[u8]) -> &[u8] {
    // Both supported native hosts use32-bit int precision. Preserve their
    // explicit C++ size_t->int narrowing, including negative precision.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "the C++ size_t->int narrowing described above"
    )]
    let precision = bytes.len() as i32;
    crate::c_str(if precision < 0 {
        bytes
    } else {
        bytes
            .get(..precision.unsigned_abs() as usize)
            .unwrap_or(bytes)
    })
}
/// The spans become slices once, here: one that is null with a length, or longer
/// than `isize::MAX`, is `None`, an error only where the kind reads it.
///
/// # Safety
/// Required input spans are initialized and disjoint from writable output storage;
/// no span or native pointer is retained. Output may be uninitialized bytes.
/// Native effects/errno run after return.
/// kind0 length,1 proxy URI,2 multipart type,3 field,4 file,5 closing boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_http_format(
    kind: u32,
    first: ClientsFormatSpan,
    second: ClientsFormatSpan,
    third: ClientsFormatSpan,
    fourth: ClientsFormatSpan,
    number: usize,
    output: *mut u8,
    capacity: usize,
) -> ClientsFormatResult {
    let output: &mut [MaybeUninit<u8>] = if capacity == 0 {
        &mut []
    } else if output.is_null() || capacity > i32::MAX as usize {
        return ClientsFormatResult {
            written: 0,
            error: 1,
        };
    } else {
        // SAFETY: the caller's `capacity` writable bytes, disjoint from every span.
        unsafe { std::slice::from_raw_parts_mut(output.cast(), capacity) }
    };
    // The spans the kind reads, the only ones the caller lends.
    let read = match kind {
        1 | 3 => 2,
        2 | 5 => 1,
        4 => 4,
        _ => 0,
    };
    let all = [first, second, third, fourth];
    let spans = core::array::from_fn(|index| {
        let &ClientsFormatSpan { data, length } = all.get(index).filter(|_| index < read)?;
        if length == 0 {
            Some(&[][..])
        } else if data.is_null() || length > isize::MAX as usize {
            None
        } else {
            // SAFETY: a span the kind reads: the caller's `length` initialized bytes.
            Some(unsafe { std::slice::from_raw_parts(data, length) })
        }
    });
    http_format(kind, spans, number, output)
}

/// `swr_v1_clients_http_format` over checked spans (`None` for an invalid one,
/// an error only where the kind reads it) into `output`.
pub fn http_format<O: Output + ?Sized>(
    kind: u32,
    spans: [Option<&[u8]>; 4],
    number: usize,
    output: &mut O,
) -> ClientsFormatResult {
    let capacity = output.capacity();
    if capacity > i32::MAX as usize {
        return ClientsFormatResult {
            written: 0,
            error: 1,
        };
    }
    let [first, second, third, fourth] = spans;
    let mut writer = Writer {
        output,
        capacity,
        count: 0,
    };
    let result = (|| -> Option<()> {
        match kind {
            0 => {
                writer.append(b"Content-Length: ")?;
                writer.decimal(number)?;
                writer.append(b"\r\n\r\n")?;
            }
            1 => {
                let host = first?;
                writer.append(b"http://")?;
                writer.append(crate::c_str(host))?;
                // Native memchr scans the full binary host before printf %s.
                if !host.contains(&b':') {
                    writer.append(b":")?;
                    writer.decimal(number)?;
                }
                writer.append(crate::c_str(second?))?;
            }
            2 => {
                writer.append(b"Content-Type: multipart/form-data; boundary=")?;
                writer.append(precision_string(first?))?;
                writer.append(b"\r\n")?;
            }
            3 | 4 => {
                writer.append(b"--")?;
                writer.append(precision_string(first?))?;
                writer.append(b"\r\nContent-Disposition: form-data; name=\"")?;
                writer.append(precision_string(second?))?;
                if kind == 4 {
                    writer.append(b"\"; filename=\"")?;
                    writer.append(precision_string(third?))?;
                    writer.append(b"\"\r\nContent-Type: ")?;
                    writer.append(precision_string(fourth?))?;
                } else {
                    writer.append(b"\"")?;
                }
                writer.append(b"\r\n\r\n")?;
            }
            5 => {
                writer.append(b"--")?;
                writer.append(precision_string(first?))?;
                writer.append(b"--\r\n")?;
            }
            _ => return None,
        }
        Some(())
    })();
    writer.finish(u32::from(result.is_none()))
}
