//! Genuine POSIX iovec progress policy. Native retains both existing arrays,
//! public counters and all payload-pointer writes; Rust reads only live lengths.
use crate::CCast;
/// What a transfer of `n` bytes does to an iovec cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IovStep {
    /// Nothing moves: no bytes, or no descriptors left.
    Unchanged,
    /// The cursor moves and descriptors remain.
    Advanced,
    /// The last descriptor completes.
    Finished,
    /// More bytes than the descriptors hold, outside the original's domain.
    Overrun,
}

pub struct ReactorIovProgress {
    pub offset_bytes: usize,
    pub advance: usize,
    pub consumed: usize,
    pub remain_count: i32,
    pub index: i32,
    pub action: IovStep,
}
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
            action: IovStep::Unchanged,
        };
    }
    let mut total = 0usize;
    for (i, length) in lengths.into_iter().enumerate() {
        total = total.wrapping_add(length);
        if total.c_cast::<isize>() >= n {
            let mut consumed = length.wrapping_sub(total.wrapping_sub(n.c_cast::<usize>()));
            let mut advance = i;
            if consumed == length {
                advance += 1;
                consumed = 0;
            }
            let remain_count = remaining.wrapping_sub(advance.c_cast::<i32>());
            return ReactorIovProgress {
                offset_bytes: (if i > 0 { 0 } else { offset }).wrapping_add(consumed),
                advance,
                consumed,
                remain_count,
                index: index.wrapping_add(advance.c_cast::<i32>()),
                action: if remain_count == 0 {
                    IovStep::Finished
                } else {
                    IovStep::Advanced
                },
            };
        }
    }
    ReactorIovProgress {
        offset_bytes: offset,
        advance: 0,
        consumed: 0,
        remain_count: remaining,
        index,
        action: IovStep::Overrun,
    }
}
