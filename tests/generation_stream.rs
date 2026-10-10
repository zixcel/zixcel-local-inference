use zixcel_cost::{CostRateCard, TokenUsage};
use zixcel_local_inference::generation_stream::{GenerationEvent, GenerationStream};

fn wire(event: &GenerationEvent) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(event).expect("event");
    bytes.push(b'\n');
    bytes
}
fn delta(sequence: u64, text: &str) -> Vec<u8> {
    wire(&GenerationEvent::Delta {
        sequence,
        text: text.into(),
    })
}
fn terminal(output: u64) -> Vec<u8> {
    wire(&GenerationEvent::Finished {
        usage: TokenUsage::new(1000, 200, 0, output, 40).expect("usage"),
    })
}

#[test]
fn byte_fragmentation_preserves_utf8_and_only_terminal_usage_is_costed() {
    let mut stream = GenerationStream::new(256).expect("stream");
    let mut bytes = delta(1, "日本語");
    bytes.extend(delta(2, "✓"));
    bytes.extend(terminal(120));
    let mut observed = String::new();
    for byte in bytes {
        stream
            .push(&[byte], |text| {
                observed.push_str(text);
                Ok(())
            })
            .expect("fragment");
    }
    let rates =
        CostRateCard::per_token("micro-JPY", "caller-test-rate", "2026-10-09", 2, 1, 3, 4, 5);
    let receipt = stream.finish(Some(&rates)).expect("complete");
    assert_eq!(receipt.text, "日本語✓");
    assert_eq!(observed, receipt.text);
    assert_eq!(receipt.usage.output_tokens, 120); // Two chunks are not two tokens.
    assert_eq!(receipt.cost.expect("cost").total_micro_units(), 2480);
    assert!(receipt.elapsed >= receipt.first_text_after.expect("first text"));
}

#[test]
fn truncated_missing_reordered_and_post_terminal_frames_never_complete() {
    for bytes in [
        delta(1, "partial"),
        delta(2, "reordered"),
        b"{\"event\":\"finished\"}".to_vec(),
        [terminal(1), delta(1, "late")].concat(),
        [terminal(1), terminal(1)].concat(),
        [terminal(1), b"unfinished".to_vec()].concat(),
        terminal(257),
        b"{\"event\":\"delta\",\"sequence\":1,\"text\":\"x\",\"extra\":1}\n".to_vec(),
    ] {
        let mut stream = GenerationStream::new(256).expect("stream");
        let _ = stream.push(&bytes, |_| Ok(()));
        assert!(stream.finish(None).is_err());
    }
}

#[test]
fn output_wire_and_receiver_failure_poison_stream_permanently() {
    for bytes in [delta(1, &"x".repeat(4097)), vec![b'x'; 65537], delta(1, "")] {
        let mut stream = GenerationStream::new(256).expect("stream");
        assert!(stream.push(&bytes, |_| Ok(())).is_err());
        assert!(stream.push(&terminal(1), |_| Ok(())).is_err());
        assert!(stream.finish(None).is_err());
    }
    let mut stream = GenerationStream::new(1).expect("stream");
    assert!(
        stream
            .push(&delta(1, "cancel"), |_| Err(
                std::io::ErrorKind::Interrupted.into()
            ))
            .is_err()
    );
    assert!(stream.finish(None).is_err());
    for n in [0, 257] {
        assert!(GenerationStream::new(n).is_err());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn crowsi_delivers_live_frames_without_python_and_eof_alone_is_not_success() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
    let address = listener.local_addr().expect("address");
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = [0; 4];
        socket.read_exact(&mut request).expect("request");
        assert_eq!(&request, b"once");
        socket.write_all(&delta(1, "first")).expect("delta");
        rx.recv_timeout(Duration::from_secs(2))
            .expect("observed before terminal");
        socket.write_all(&terminal(1)).expect("terminal");
    });
    let mut decoder = GenerationStream::new(1).expect("decoder");
    crowsi_transport_foundation::process::guarded_tcp_stream(
        address,
        b"once",
        65536,
        Instant::now() + Duration::from_secs(3),
        || Ok(()),
        |bytes| {
            decoder
                .push(bytes, |text| {
                    assert_eq!(text, "first");
                    tx.send(()).expect("ack");
                    Ok(())
                })
                .map_err(|_| std::io::ErrorKind::InvalidData.into())
        },
    )
    .expect("transport");
    assert_eq!(decoder.finish(None).expect("complete").text, "first");
    worker.join().expect("worker");
}
