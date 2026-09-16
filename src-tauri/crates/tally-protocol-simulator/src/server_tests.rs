use super::*;

#[test]
fn tally_xml_content_type_classifiers_are_exact_and_case_insensitive() {
    assert!(has_tally_xml_utf16_content_type(
        b"POST / HTTP/1.1\r\ncontent-type: TEXT/XML; CHARSET=UTF-16\r\n\r\n<E />"
    ));
    assert!(!has_plain_tally_xml_content_type(
        b"POST / HTTP/1.1\r\ncontent-type: TEXT/XML; CHARSET=UTF-16\r\n\r\n<E />"
    ));
    assert!(has_plain_tally_xml_content_type(
        b"GET /status HTTP/1.1\r\ncontent-type: TEXT/XML\r\n\r\n"
    ));
    assert!(!has_tally_xml_utf16_content_type(
        b"GET /status HTTP/1.1\r\ncontent-type: TEXT/XML\r\n\r\n"
    ));
    assert!(!has_tally_xml_utf16_content_type(
        b"POST / HTTP/1.1\r\nContent-Type: application/xml; charset=utf-8\r\n\r\n<E />"
    ));
    assert!(!has_tally_xml_utf16_content_type(
        b"POST / HTTP/1.1\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Type: application/xml\r\n\r\n<E />"
    ));
}

#[test]
fn read_request_enforces_deadline_while_a_peer_drip_feeds_bytes() {
    let listener = bind_loopback_listener().expect("bind loopback listener");
    let address = listener.local_addr().expect("read listener address");
    let writer = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).expect("connect loopback listener");
        stream.set_nodelay(true).expect("disable Nagle buffering");
        for _ in 0..10 {
            if stream.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
    });
    let (mut stream, _) = listener.accept().expect("accept loopback peer");
    stream
        .set_read_timeout(Some(Duration::from_millis(25)))
        .expect("set read poll timeout");
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();

    let error = read_request(&mut stream, &cancelled, Duration::from_millis(70))
        .expect_err("incomplete drip feed must not outlive its deadline");

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "deadline enforcement must stay bounded"
    );
    writer.join().expect("drip-feed writer does not panic");
}
