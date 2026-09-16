use super::*;
use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator, WireEncoding};

#[tokio::test]
async fn sends_only_the_rendered_sealed_read_profile() {
    let profile = ReadOnlyProfile::CompanyListV1;
    let expected = profile.render();
    for attempt in 1..=5 {
        let simulator = Simulator::spawn(
            ScenarioPlan::new(Fixture::EmptyExport).with_encoding(WireEncoding::Utf16Le),
        )
        .unwrap();
        let transport =
            ReadOnlyTransport::new(ReadLoopback::Ipv4, simulator.address().port()).unwrap();
        match transport.send(profile).await {
            Ok(response) => {
                assert_eq!(response.http_status(), 200);
                let observed = simulator.finish().unwrap();
                assert_eq!(observed.method, "POST");
                assert_eq!(observed.path, "/");
                assert!(observed.request_processed);
                assert!(observed.bytes_received > expected.len());
                return;
            }
            Err(error) if attempt < 5 && error.safe_code() == "request_failed" => {
                // Windows endpoint-security software can abort a newly opened synthetic
                // loopback flow before the request reaches the listener. Recreate only the
                // test fixture; the production read transport remains single-attempt.
                drop(simulator);
            }
            Err(error) => panic!("sealed read fixture failed: {error:?}"),
        }
    }
    unreachable!("bounded fixture attempts always return")
}
