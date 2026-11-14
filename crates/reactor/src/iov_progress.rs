//! Genuine POSIX iovec progress policy. Native retains both existing arrays,
//! public counters and all payload-pointer writes; Rust reads only live lengths.
#[repr(C)]
pub struct ReactorIovProgress {
    pub offset_bytes: usize,
    pub advance: usize,
    pub consumed: usize,
    pub remain_count: i32,
    pub index: i32,
    pub action: i32,
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "native ssize_t/int iovec progress arithmetic"
)]
pub(crate) fn progress(
    lengths: impl IntoIterator<Item = usize>,
    n: isize,
    remaining: i32,
    index: i32,
    offset: usize,
) -> ReactorIovProgress {
    if n <= 0 || remaining == 0 {
        return ReactorIovProgress {
            offset_bytes: offset,
            advance: 0,
            consumed: 0,
            remain_count: remaining,
            index,
            action: 0,
        };
    }
    let mut total = 0usize;
    for (i, length) in lengths.into_iter().enumerate() {
        total = total.wrapping_add(length);
        if (total as isize) >= n {
            let mut consumed = length.wrapping_sub(total.wrapping_sub(n as usize));
            let mut advance = i;
            if consumed == length {
                advance += 1;
                consumed = 0;
            }
            let remain_count = remaining.wrapping_sub(advance as i32);
            return ReactorIovProgress {
                offset_bytes: (if i > 0 { 0 } else { offset }).wrapping_add(consumed),
                advance,
                consumed,
                remain_count,
                index: index.wrapping_add(advance as i32),
                action: if remain_count == 0 { 2 } else { 1 },
            };
        }
    }
    ReactorIovProgress {
        offset_bytes: offset,
        advance: 0,
        consumed: 0,
        remain_count: remaining,
        index,
        action: -1,
    }
}
