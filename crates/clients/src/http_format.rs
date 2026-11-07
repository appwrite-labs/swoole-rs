//! Allocation-free request formatting into disjoint native-owned storage.
//! Byte/C-string behavior and snprintf's clamped length/terminator are explicit.
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
struct Writer {
    output: *mut u8,
    capacity: usize,
    count: usize,
}
impl Writer {
    fn append(&mut self, bytes: &[u8]) -> Option<()> {
        let count = self.count.checked_add(bytes.len())?;
        let offset = self.count.min(self.capacity.saturating_sub(1));
        let copied = bytes
            .len()
            .min(self.capacity.saturating_sub(1).saturating_sub(offset));
        if copied != 0 {
            // SAFETY: `offset + copied <= capacity - 1`, inside the caller's writable output,
            // which is disjoint from every input span.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.output.wrapping_add(offset),
                    copied,
                );
            };
        }
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
            // SAFETY: `written <= capacity - 1` (or 0), inside the writable output.
            unsafe {
                self.output
                    .wrapping_add(if error == 0 { written } else { 0 })
                    .write(0);
            };
        }
        ClientsFormatResult { written, error }
    }
}
unsafe fn span<'a>(value: ClientsFormatSpan) -> Option<&'a [u8]> {
    if value.length == 0 {
        Some(&[])
    } else if value.data.is_null() || value.length > isize::MAX as usize {
        None
    } else {
        // SAFETY: `value.data` is non-null and `value.length <= isize::MAX` (checked above); the caller lends `value.length` initialized bytes for this call.
        Some(unsafe { std::slice::from_raw_parts(value.data, value.length) })
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
/// # Safety
/// Required input spans are initialized and disjoint from writable output storage;
/// no span or native pointer is retained. Output may be uninitialized bytes and
/// is written through raw pointers only. Native effects/errno run after return.
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
    if capacity > i32::MAX as usize || (capacity != 0 && output.is_null()) {
        return ClientsFormatResult {
            written: 0,
            error: 1,
        };
    }
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
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                let host = unsafe { span(first) }?;
                writer.append(b"http://")?;
                writer.append(crate::c_str(host))?;
                // Native memchr scans the full binary host before printf %s.
                if !host.contains(&b':') {
                    writer.append(b":")?;
                    writer.decimal(number)?;
                }
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                writer.append(crate::c_str(unsafe { span(second) }?))?;
            }
            2 => {
                writer.append(b"Content-Type: multipart/form-data; boundary=")?;
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                writer.append(precision_string(unsafe { span(first) }?))?;
                writer.append(b"\r\n")?;
            }
            3 | 4 => {
                writer.append(b"--")?;
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                writer.append(precision_string(unsafe { span(first) }?))?;
                writer.append(b"\r\nContent-Disposition: form-data; name=\"")?;
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                writer.append(precision_string(unsafe { span(second) }?))?;
                if kind == 4 {
                    writer.append(b"\"; filename=\"")?;
                    // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                    writer.append(precision_string(unsafe { span(third) }?))?;
                    writer.append(b"\"\r\nContent-Type: ")?;
                    // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                    writer.append(precision_string(unsafe { span(fourth) }?))?;
                } else {
                    writer.append(b"\"")?;
                }
                writer.append(b"\r\n\r\n")?;
            }
            5 => {
                writer.append(b"--")?;
                // SAFETY: required spans name `length` initialized bytes, disjoint from the output.
                writer.append(precision_string(unsafe { span(first) }?))?;
                writer.append(b"--\r\n")?;
            }
            _ => return None,
        }
        Some(())
    })();
    writer.finish(u32::from(result.is_none()))
}
