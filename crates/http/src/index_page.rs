//! Directory-index markup and filtering.
use crate::c_text;

/// The texts one page part shows.
pub(crate) struct HttpIndexPage<'a> {
    pub(crate) path: &'a [u8],
    pub(crate) entry: &'a [u8],
    pub(crate) css: &'a [u8],
    pub(crate) powered: &'a [u8],
}

/// Append one page part to `out`: 0=open, 1=directory entry, 2=close; another part
/// appends nothing. Entry filtering uses full strings; displayed %s values stop at
/// NUL. Legacy markup is intentionally unescaped. No URL/HTML policy change is made.
/// A part beyond `format_impl`'s int, or one `out` cannot grow by, appends nothing,
/// as upstream's failed append.
pub(crate) fn index_page(out: &mut Vec<u8>, part: u32, page: &HttpIndexPage<'_>) {
    let HttpIndexPage {
        path,
        entry,
        css,
        powered,
    } = *page;
    let skip = part == 1 && (entry == b"." || path == b"/" && entry == b"..");
    let pieces: &[&[u8]] = match part {
        0 => &[
            b"<html>\n<head>\n\t<meta charset='UTF-8'>\n<title>Index of ",
            c_text(path),
            b"</title></head>\n<body>\n",
            css,
            b"<h1>Index of ",
            c_text(path),
            b"</h1>\t<ul>\n",
        ],
        1 if !skip => &[
            b"\t\t<li><a href=",
            c_text(path),
            c_text(entry),
            b">",
            c_text(entry),
            b"</a></li>\n",
        ],
        2 => &[b"\t</ul>\n", powered, b"</body>\n</html>\n"],
        _ => &[],
    };
    // Native format_impl's result is int. Reject unsupported huge fragments
    // before mutation; ordinary page totals remain native size_t.
    let fits = pieces
        .iter()
        .try_fold(0usize, |sum, piece| sum.checked_add(piece.len()))
        .filter(|&n| i32::try_from(n).is_ok())
        .is_some_and(|total| out.try_reserve(total).is_ok());
    if fits {
        for piece in pieces {
            out.extend_from_slice(piece);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page<'a>(
        path: &'a [u8],
        entry: &'a [u8],
        css: &'a [u8],
        powered: &'a [u8],
    ) -> HttpIndexPage<'a> {
        HttpIndexPage {
            path,
            entry,
            css,
            powered,
        }
    }
    #[test]
    fn parts_append_to_existing_bytes() {
        let page = page(b"/ordinary/", b"a.txt", b"css", b"powered");
        let expected: [&[u8]; 3] = [
            b"<html>\n<head>\n\t<meta charset='UTF-8'>\n<title>Index of /ordinary/</title></head>\n<body>\ncss<h1>Index of /ordinary/</h1>\t<ul>\n",
            b"\t\t<li><a href=/ordinary/a.txt>a.txt</a></li>\n",
            b"\t</ul>\npowered</body>\n</html>\n",
        ];
        for (part, expected) in (0..).zip(expected) {
            let mut output = b"prefix".to_vec();
            index_page(&mut output, part, &page);
            assert_eq!(&output[..6], b"prefix");
            assert_eq!(&output[6..], expected);
        }
        let mut output = b"prefix".to_vec();
        index_page(&mut output, 3, &page);
        assert_eq!(output, b"prefix");
    }
    #[test]
    fn native_filtering_and_c_string_display_are_distinct() {
        let mut output = Vec::new();
        index_page(&mut output, 1, &page(b"/", b"..", b"", b""));
        index_page(&mut output, 1, &page(b"/x/", b".", b"", b""));
        assert_eq!(output, b"");
        index_page(&mut output, 1, &page(b"/", b"..\0name", b"", b""));
        assert_eq!(output, b"\t\t<li><a href=/..>..</a></li>\n");
    }
}
