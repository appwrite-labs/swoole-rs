//! Allocation-free request formatting into the caller's buffer.
//! Byte/C-string behavior and snprintf's clamped length/terminator are explicit.
use swoole_rs_corebase::CCast;

pub struct ClientsFormatResult {
    pub written: usize,
    ///0 success;1 invalid span/output;2 printf count overflow (native errno).
    pub error: u32,
}
/// Where a request is formatted.
pub trait Output {
    fn capacity(&self) -> usize;
    /// Writes `bytes` at `offset`, which the writer keeps inside the capacity.
    fn put(&mut self, offset: usize, bytes: &[u8]);
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
    let precision = bytes.len().c_cast::<i32>();
    crate::c_str(if precision < 0 {
        bytes
    } else {
        bytes
            .get(..precision.unsigned_abs() as usize)
            .unwrap_or(bytes)
    })
}
/// The request formatter's `sw_snprintf` call for `kind` over `spans` (`None` for an
/// invalid one, an error only where the kind reads it) and `number`, into `output`.
/// kind0 length,1 proxy URI,2 multipart type,3 field,4 file,5 closing boundary.
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
