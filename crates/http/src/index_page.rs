//! Directory-index markup and filtering.
use crate::{c_text, encoded_url_bytes, static_meta::append_parts};

fn html_escape(text: &[u8]) -> Vec<u8> {
    text.iter()
        .flat_map(|byte| match byte {
            b'&' => b"&amp;".as_slice(),
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&#39;",
            _ => core::slice::from_ref(byte),
        })
        .copied()
        .collect()
}

/// Append upstream's header, visible directory entries and footer. Each formatted
/// fragment must fit the native formatter's signed result.
pub(crate) fn index_page<'a>(
    out: &mut Vec<u8>,
    path: &[u8],
    entries: impl Iterator<Item = &'a [u8]>,
    css: &[u8],
    powered: &[u8],
) {
    let escaped = html_escape(path);
    let shown = c_text(&escaped);
    let mut append = |parts: &[&[u8]]| {
        _ = append_parts(out, parts, i32::MAX as usize);
    };
    append(&[
        b"<html>\n<head>\n\t<meta charset='UTF-8'>\n<title>Index of ",
        shown,
        b"</title></head>\n<body>\n",
        css,
        b"<h1>Index of ",
        shown,
        b"</h1>\t<ul>\n",
    ]);
    for entry in entries {
        if entry == b"." || path == b"/" && entry == b".." {
            continue;
        }
        let segment: Vec<u8> = encoded_url_bytes(entry).collect();
        append(&[
            b"\t\t<li><a href=\"",
            shown,
            if shown.len() == escaped.len() {
                &segment
            } else {
                b""
            },
            b"\">",
            &html_escape(c_text(entry)),
            b"</a></li>\n",
        ]);
    }
    append(&[b"\t</ul>\n", powered, b"</body>\n</html>\n"]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_links_encode_bytes_and_labels_escape_html() {
        let mut page = b"prefix".to_vec();
        let names = [b".".as_slice(), b"..", b"Q&A <2026> \"draft\" 'a'.txt"];
        index_page(
            &mut page,
            b"/reports&notes/",
            names.into_iter(),
            b"css",
            b"powered",
        );
        let page = String::from_utf8(page).unwrap();
        assert!(page.starts_with("prefix<html>"));
        assert!(page.contains("<title>Index of /reports&amp;notes/</title>"));
        assert!(page.contains("<h1>Index of /reports&amp;notes/</h1>"));
        assert!(page.contains("href=\"/reports&amp;notes/Q%26A%20%3C2026%3E%20%22draft%22%20%27a%27.txt\">Q&amp;A &lt;2026&gt; &quot;draft&quot; &#39;a&#39;.txt</a>"));
        assert!(page.contains("href=\"/reports&amp;notes/..\">..</a>"));
        assert_eq!(page.matches("<li>").count(), 2);
    }

    #[test]
    fn root_filters_full_names_and_displays_labels_until_nul() {
        let mut page = Vec::new();
        index_page(
            &mut page,
            b"/",
            [b"..".as_slice(), b"..\0name"].into_iter(),
            b"",
            b"",
        );
        let page = String::from_utf8(page).unwrap();
        assert_eq!(page.matches("<li>").count(), 1);
        assert!(page.contains("href=\"/..%00name\">..</a>"));
    }
}
