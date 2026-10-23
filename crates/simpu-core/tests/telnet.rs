#![allow(clippy::unwrap_used)]

use simpu_core::telnet::{TelnetEvent, TelnetSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const IAC: u8 = 255;

/// Minimal telnet server: asks for NAWS, prints a banner, reports what it receives.
#[tokio::test]
async fn negotiation_banner_input_and_resize() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        s.write_all(&[IAC, 253, 31]).await.unwrap(); // DO NAWS
        s.write_all(b"login: ").await.unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        // expect WILL NAWS + SB NAWS(80x24), then "root\r\0", then SB NAWS(100x30)
        while got.len() < 3 + 9 + 6 + 9 {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "client closed early: {got:?}");
            got.extend_from_slice(&buf[..n]);
        }
        got
    });

    let mut t = TelnetSession::connect("127.0.0.1", port, 80, 24)
        .await
        .unwrap();
    let mut banner = Vec::new();
    while !banner.ends_with(b"login: ") {
        match t.next().await {
            TelnetEvent::Data(d) => banner.extend(d),
            TelnetEvent::Closed => panic!("closed"),
        }
    }
    assert_eq!(banner, b"login: ");
    let w = t.writer();
    w.write(b"root\r").await.unwrap();
    w.resize(100, 30).await.unwrap();

    let got = server.await.unwrap();
    let mut want = vec![IAC, 251, 31, IAC, 250, 31, 0, 80, 0, 24, IAC, 240];
    want.extend(b"root\r\0");
    want.extend([IAC, 250, 31, 0, 100, 0, 30, IAC, 240]);
    assert_eq!(got, want);
}

#[tokio::test]
async fn close_is_reported() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        drop(s);
    });
    let mut t = TelnetSession::connect("127.0.0.1", port, 80, 24)
        .await
        .unwrap();
    assert_eq!(t.next().await, TelnetEvent::Closed);
}
