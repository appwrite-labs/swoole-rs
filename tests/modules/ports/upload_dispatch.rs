#[test]
fn upload_files_survive_dispatch_only_when_accepted() {
    const CONTENTS: &[u8] = &[b'x'; 512];

    fn path(packet: &[u8]) -> &std::path::Path {
        let name = std::str::from_utf8(packet)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("Swoole-Upload-File: "))
            .unwrap();
        std::path::Path::new(name)
    }

    fn dispatch<const ACCEPT: bool>(packet: &[u8]) -> bool {
        assert_eq!(std::fs::read(path(packet)).unwrap(), CONTENTS);
        ACCEPT
    }

    let body = [
        &b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\n"[..],
        CONTENTS,
        b"\r\n--b--\r\n",
    ]
    .concat();
    let head = format!(
        "POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    for accepted in [false, true] {
        let mut f = fake(&[&[head.as_bytes(), &body].concat()]);
        f.buf.extend(1024);
        f.max = 350;
        f.uploads = 1024;
        f.on_dispatch = if accepted {
            dispatch::<true>
        } else {
            dispatch::<false>
        };
        run(Readable::Http, &mut f);
        let [packet] = f.dispatched.as_slice() else {
            panic!("expected one upload dispatch");
        };
        let file = path(packet);
        if accepted {
            assert_eq!(std::fs::read(file).unwrap(), CONTENTS);
            std::fs::remove_file(file).unwrap();
        } else {
            assert_eq!(
                std::fs::metadata(file).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }
    }
}
