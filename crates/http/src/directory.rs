//! Private `StaticHandler` directory owner. No native container or name mirror.
use crate::proxy::HttpProxyText;
use crate::{HttpIndexPage, INVALID, bytes, swr_v1_http_index_page};
use std::{
    alloc::{Layout, alloc},
    ptr,
};

#[derive(Default)]
pub struct HttpDirectory {
    names: Vec<std::ops::Range<usize>>,
    bytes: Vec<u8>,
    pending: bool,
}
const ALLOCATION: i32 = -3;

impl HttpDirectory {
    fn name(&self, index: usize) -> Option<&[u8]> {
        self.names.get(index).map(|span| &self.bytes[span.clone()])
    }
    fn copy_name(&mut self, name: &[u8]) -> Result<std::ops::Range<usize>, i32> {
        self.names.try_reserve(1).map_err(|_| ALLOCATION)?;
        self.bytes.try_reserve(name.len()).map_err(|_| ALLOCATION)?;
        let start = self.bytes.len();
        self.bytes.extend_from_slice(name);
        Ok(start..self.bytes.len())
    }
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }
    pub(crate) fn contains(&self, name: &[u8]) -> Result<bool, i32> {
        if self.pending {
            return Err(INVALID);
        }
        Ok(self
            .names
            .binary_search_by(|entry| self.bytes[entry.clone()].cmp(name))
            .is_ok())
    }
    pub(crate) fn scan_append(&mut self, name: &[u8]) -> Result<(), i32> {
        let span = self.copy_name(name)?;
        self.names.push(span);
        self.pending = true;
        Ok(())
    }
    pub(crate) fn scan_finish(&mut self) {
        if self.pending {
            let bytes = &self.bytes;
            self.names
                .sort_unstable_by(|a, b| bytes[a.clone()].cmp(&bytes[b.clone()]));
            self.names
                .dedup_by(|a, b| bytes[a.clone()] == bytes[b.clone()]);
            self.pending = false;
        }
    }
}

/// Allocate the sole directory owner, or null on allocation failure. Empty
/// storage allocates no names. Native construction/exception effects follow return.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v2_http_directory_new() -> *mut HttpDirectory {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<HttpDirectory>()"
    )]
    // SAFETY: a non-zero-sized layout; null is handled below.
    let owner = unsafe { alloc(Layout::new::<HttpDirectory>()).cast::<HttpDirectory>() };
    if !owner.is_null() {
        // SAFETY: `owner` is a fresh, non-null allocation with its type's layout.
        unsafe { owner.write(HttpDirectory::default()) };
    }
    owner
}
/// # Safety
/// Owner is null or a live unique result of `directory_new`, destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_drop(owner: *mut HttpDirectory) {
    if !owner.is_null() {
        // SAFETY: a live, unique owner from `directory_new`, destroyed once.
        unsafe { drop(Box::from_raw(owner)) };
    }
}
/// Deep-copy a finished value without publishing a partial owner. Null denotes
/// lazy empty storage. Native copy construction/assignment effects follow return.
/// # Safety
/// Source is null or live, finished and unchanged; destination is aligned,
/// writable and disjoint from source/storage, and does not already own a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_clone(
    source: *const HttpDirectory,
    destination: *mut *mut HttpDirectory,
) -> i32 {
    // SAFETY: `destination` is null or live, aligned and exclusively lent for this call.
    let Some(destination) = (unsafe { destination.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: `source` is null or live and aligned for this call.
    let Some(source) = (unsafe { source.as_ref() }) else {
        *destination = ptr::null_mut();
        return 0;
    };
    if source.pending {
        return INVALID;
    }
    let mut copy = HttpDirectory::default();
    if copy.names.try_reserve_exact(source.names.len()).is_err()
        || copy.bytes.try_reserve_exact(source.bytes.len()).is_err()
    {
        return ALLOCATION;
    }
    copy.names.extend_from_slice(&source.names);
    copy.bytes.extend_from_slice(&source.bytes);
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<HttpDirectory>()"
    )]
    // SAFETY: a non-zero-sized layout; null is handled below.
    let owner = unsafe { alloc(Layout::new::<HttpDirectory>()).cast::<HttpDirectory>() };
    if owner.is_null() {
        return ALLOCATION;
    }
    // SAFETY: `owner` is a fresh, non-null allocation with its type's layout.
    unsafe { owner.write(copy) };
    *destination = owner;
    0
}
/// Insert a copied complete name. Unsigned byte ordering and duplicate suppression
/// match native `std::set`<std::string>. Returns1=inserted,0=already present.
/// All allocation is fallible; errors publish no name. No native call occurs.
/// # Safety
/// Live unique owner; initialized input span, disjoint from owner storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_insert(
    owner: *mut HttpDirectory,
    data: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let name = match unsafe { bytes(data, length) } {
        Ok(name) => name,
        Err(error) => return error,
    };
    if owner.pending {
        return INVALID;
    }
    let Err(index) = owner
        .names
        .binary_search_by(|entry| owner.bytes[entry.clone()].cmp(name))
    else {
        return 0;
    };
    let span = match owner.copy_name(name) {
        Ok(span) => span,
        Err(error) => return error,
    };
    owner.names.insert(index, span);
    1
}
/// Append copied scan input without per-entry ordered-vector shifting. This
/// starts/continues a private unfinished scan; only append, finish and drop may
/// consume the owner until finish returns. Returns1 or a negative error. A failed
/// append publishes no name; previously appended inputs remain owned.
/// # Safety
/// Live unique owner with initialized disjoint input; no observer or existing
/// ordered insertion may run before finish. No borrow spans native readdir.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_scan_append(
    owner: *mut HttpDirectory,
    data: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let name = match unsafe { bytes(data, length) } {
        Ok(name) => name,
        Err(error) => return error,
    };
    match owner.scan_append(name) {
        Ok(()) => 1,
        Err(error) => error,
    }
}
/// Finish the private scan in place, without another name store or allocation:
/// unsigned byte sort then duplicate destruction. Null is an empty lazy scan.
/// Native closedir completes before this call; no native effects occur in Rust.
/// # Safety
/// Null or a live unique owner. Existing observers are valid only after return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_scan_finish(owner: *mut HttpDirectory) {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    if let Some(owner) = unsafe { owner.as_mut() } {
        owner.scan_finish();
    }
}
/// # Safety
/// Owner is null (lazy empty) or live, finished and unchanged for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_count(owner: *const HttpDirectory) -> usize {
    // SAFETY: `owner` is null or live and aligned for this call.
    unsafe { owner.as_ref() }.map_or(0, HttpDirectory::len)
}
/// # Safety
/// Null (lazy empty) or live finished unchanged owner, initialized independent name span. Returns1/0 or error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_contains(
    owner: *const HttpDirectory,
    data: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let name = match unsafe { bytes(data, length) } {
        Ok(name) => name,
        Err(error) => return error,
    };
    // SAFETY: `owner` is null or live and aligned for this call.
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return 0;
    };
    match owner.contains(name) {
        Ok(found) => i32::from(found),
        Err(error) => error,
    }
}
/// Render directly from owned ordered name bytes, without exporting a borrowed
/// pointer or making a persistent native copy. Part0/2 ignore index; part1 uses
/// the indexed directory entry. Existing page serialization/size policy is shared.
/// # Safety
/// Null empty or live finished unchanged owner and initialized page; part1 requires a live
/// indexed entry. Output/written obey `index_page`'s
/// disjointness contract and cannot alias this owner's storage. No borrow survives
/// native reserve/append/IO; query and fill are separate complete calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_page(
    owner: *const HttpDirectory,
    index: usize,
    page: *const HttpIndexPage,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: `page` is null or live and aligned for this call.
    let Some(page) = (unsafe { page.as_ref() }) else {
        return INVALID;
    };
    let entry = if page.part == 1 {
        // SAFETY: `owner` is null or live and aligned for this call.
        let Some(owner) = (unsafe { owner.as_ref() }) else {
            return INVALID;
        };
        if owner.pending {
            return INVALID;
        }
        let Some(name) = owner.name(index) else {
            return INVALID;
        };
        HttpProxyText {
            data: name.as_ptr(),
            length: name.len(),
        }
    } else {
        HttpProxyText {
            data: ptr::null(),
            length: 0,
        }
    };
    let copy_span = |text: &HttpProxyText| HttpProxyText {
        data: text.data,
        length: text.length,
    };
    let page = HttpIndexPage {
        path: copy_span(&page.path),
        entry,
        css: copy_span(&page.css),
        powered: copy_span(&page.powered),
        part: page.part,
    };
    // SAFETY: `page` is a local copy of the caller's spans; output follows its contract.
    unsafe { swr_v1_http_index_page(&raw const page, output, capacity, written) }
}

/// Complete page append policy over the sole ordered directory owner. Actions
/// and buffer publication match `context_page_emit`; no name pointer escapes.
/// # Safety
/// Owner is null empty or live finished/unchanged; input spans, buffer and output
/// obey `index_page_emit`'s disjoint initialized-input/writable-backing contract.
/// No borrow survives a Native reserve/extend; reobserve backing on each call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_directory_emit(
    owner: *const HttpDirectory,
    index: usize,
    input: *const HttpIndexPage,
    buffer: *mut u8,
    length: usize,
    capacity: usize,
    output: *mut crate::HttpContextPageBuffer,
) -> i32 {
    let render = |data, size, written| {
        // SAFETY: `owner` and `input` follow the export's contract; emit_page passes a
        // writable span.
        unsafe { swr_v2_http_directory_page(owner, index, input, data, size, written) }
    };
    // SAFETY: forwards the caller's input, buffer and output under emit_page's contract.
    unsafe { crate::index_page::emit_page(input, None, buffer, length, capacity, output, render) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn legacy_value_copy_and_direct_page_emission() {
        unsafe {
            let mut lazy = ptr::null_mut();
            assert_eq!(swr_v2_http_directory_clone(ptr::null(), &raw mut lazy), 0);
            assert!(lazy.is_null());
            for count in [1, 17, 65, 257] {
                let owner = swr_v2_http_directory_new();
                assert!(!owner.is_null());
                let mut expected = BTreeSet::new();
                for i in (0..count).rev() {
                    let name = format!("file-{i:04}-é&.txt").into_bytes();
                    assert_eq!(
                        swr_v2_http_directory_insert(owner, name.as_ptr(), name.len()),
                        1
                    );
                    assert_eq!(
                        swr_v2_http_directory_insert(owner, name.as_ptr(), name.len()),
                        0
                    );
                    expected.insert(name);
                }
                let mut copy = ptr::null_mut();
                assert_eq!(swr_v2_http_directory_clone(owner, &raw mut copy), 0);
                assert!(!copy.is_null());
                assert_ne!(owner, copy);
                assert_eq!((*owner).names, (*copy).names);
                assert_eq!((*owner).bytes, (*copy).bytes);
                for i in 0..(*owner).len() {
                    assert_ne!(
                        (*owner).name(i).unwrap().as_ptr(),
                        (*copy).name(i).unwrap().as_ptr()
                    );
                }
                let extra = b"separate.txt";
                assert_eq!(
                    swr_v2_http_directory_insert(copy, extra.as_ptr(), extra.len()),
                    1
                );
                assert_eq!(
                    swr_v2_http_directory_contains(owner, extra.as_ptr(), extra.len()),
                    0
                );
                swr_v2_http_directory_drop(copy);
                let span = |value: &[u8]| HttpProxyText {
                    data: value.as_ptr(),
                    length: value.len(),
                };
                for (index, name) in expected.iter().enumerate() {
                    let page = HttpIndexPage {
                        path: span(b"/normal/"),
                        entry: span(name),
                        css: span(b"css"),
                        powered: span(b"power"),
                        part: 1,
                    };
                    let mut written = 0;
                    assert_eq!(
                        swr_v1_http_index_page(
                            &raw const page,
                            ptr::null_mut(),
                            0,
                            &raw mut written
                        ),
                        0
                    );
                    let mut rendered = vec![0; written];
                    assert_eq!(
                        swr_v1_http_index_page(
                            &raw const page,
                            rendered.as_mut_ptr(),
                            rendered.len(),
                            &raw mut written
                        ),
                        0
                    );
                    let mut buffer = vec![0xa5; written + 4 + 8];
                    let mut result = crate::HttpContextPageBuffer::default();
                    assert_eq!(
                        swr_v2_http_directory_emit(
                            owner,
                            index,
                            &raw const page,
                            buffer.as_mut_ptr(),
                            3,
                            buffer.len() - 8,
                            &raw mut result
                        ),
                        3
                    );
                    assert_eq!(&buffer[..3], &[0xa5; 3]);
                    assert_eq!(&buffer[3..3 + written], rendered);
                    assert_eq!(result.length, written + 3);
                    assert_eq!(buffer[result.length], 0);
                    assert_eq!(&buffer[result.length + 1..], &[0xa5; 8]);
                }
                swr_v2_http_directory_drop(owner);
            }
        }
    }
    #[test]
    fn sole_owned_sorted_names_deduplicate_and_survive_input_reuse() {
        let owner = swr_v2_http_directory_new();
        assert!(!owner.is_null());
        let mut expected = BTreeSet::new();
        for i in (0..1024u32).rev() {
            let mut name = format!("entry-{i:04}-é").into_bytes();
            assert_eq!(
                unsafe { swr_v2_http_directory_insert(owner, name.as_ptr(), name.len()) },
                1
            );
            expected.insert(name.clone());
            assert_eq!(
                unsafe { swr_v2_http_directory_insert(owner, name.as_ptr(), name.len()) },
                0
            );
            name.fill(0xcc);
        }
        assert_eq!(
            unsafe { swr_v2_http_directory_count(owner) },
            expected.len()
        );
        assert_eq!(
            (0..unsafe { &*owner }.len())
                .map(|i| unsafe { &*owner }.name(i).unwrap())
                .collect::<Vec<_>>(),
            expected.iter().map(Vec::as_slice).collect::<Vec<_>>()
        );
        for name in expected {
            assert_eq!(
                unsafe { swr_v2_http_directory_contains(owner, name.as_ptr(), name.len()) },
                1
            );
        }
        unsafe { swr_v2_http_directory_drop(owner) };
    }
    #[test]
    fn scan_finishes_same_owner_with_sorted_unique_copied_bytes() {
        let owner = swr_v2_http_directory_new();
        assert!(!owner.is_null());
        let mut expected = BTreeSet::new();
        for i in (0..4096u32).rev() {
            let mut name = format!("entry-{i:04}-é").into_bytes();
            for _ in 0..2 {
                assert_eq!(
                    unsafe { swr_v2_http_directory_scan_append(owner, name.as_ptr(), name.len()) },
                    1
                );
            }
            expected.insert(name.clone());
            name.fill(0xcc);
        }
        assert_eq!(
            unsafe { swr_v2_http_directory_insert(owner, b"x".as_ptr(), 1) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v2_http_directory_contains(owner, b"x".as_ptr(), 1) },
            INVALID
        );
        unsafe { swr_v2_http_directory_scan_finish(owner) };
        assert_eq!(
            (0..unsafe { &*owner }.len())
                .map(|i| unsafe { &*owner }.name(i).unwrap())
                .collect::<Vec<_>>(),
            expected.iter().map(Vec::as_slice).collect::<Vec<_>>()
        );
        assert_eq!(
            unsafe { swr_v2_http_directory_count(owner) },
            expected.len()
        );
        unsafe { swr_v2_http_directory_scan_finish(owner) };
        assert_eq!(
            unsafe { swr_v2_http_directory_insert(owner, b"x".as_ptr(), 1) },
            1
        );
        assert_eq!(
            unsafe { swr_v2_http_directory_contains(owner, b"x".as_ptr(), 1) },
            1
        );
        unsafe { swr_v2_http_directory_drop(owner) };
        unsafe { swr_v2_http_directory_scan_finish(ptr::null_mut()) };
    }
    #[test]
    fn rendering_reads_owned_entry_and_preserves_page_query_prefix_contract() {
        let owner = swr_v2_http_directory_new();
        assert!(!owner.is_null());
        let span = |bytes: &[u8]| HttpProxyText {
            data: bytes.as_ptr(),
            length: bytes.len(),
        };
        let name = b"a & <name>";
        assert_eq!(
            unsafe { swr_v2_http_directory_insert(owner, name.as_ptr(), name.len()) },
            1
        );
        let page = HttpIndexPage {
            path: span(b"/dir/"),
            entry: span(b"ignored"),
            css: span(b""),
            powered: span(b""),
            part: 1,
        };
        let expected = b"\t\t<li><a href=/dir/a & <name>>a & <name></a></li>\n";
        for capacity in 0..=expected.len() + 2 {
            let mut output = vec![0xcc; capacity + 1];
            let mut written = 99;
            assert_eq!(
                unsafe {
                    swr_v2_http_directory_page(
                        owner,
                        0,
                        &raw const page,
                        output.as_mut_ptr(),
                        capacity,
                        &raw mut written,
                    )
                },
                0
            );
            assert_eq!(written, expected.len());
            assert_eq!(
                &output[..capacity.min(written)],
                &expected[..capacity.min(written)]
            );
            assert_eq!(output[capacity], 0xcc);
        }
        unsafe { swr_v2_http_directory_drop(owner) };
    }
}
