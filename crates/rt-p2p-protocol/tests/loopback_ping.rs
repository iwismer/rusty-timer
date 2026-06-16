use std::{error::Error, time::Duration};

use bytes::BytesMut;
use rt_iroh::EndpointBuilder;
use rt_p2p_protocol::{
    Hello, HelloOk, MAX_FRAME_BYTES, decode_message_frame, encode_frame, negotiate,
};
const EXPECTED_MINOR: u32 = 3;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test]
async fn loopback_endpoint_pair_exchanges_framed_hello() -> TestResult {
    match tokio::time::timeout(Duration::from_secs(10), exchange_hello()).await {
        Ok(result) => result,
        Err(error) => Err(Box::new(error) as Box<dyn Error + Send + Sync>),
    }
}

async fn exchange_hello() -> TestResult {
    let server = EndpointBuilder::test([10; 32]).bind().await?;
    let client = EndpointBuilder::test([20; 32]).bind().await?;
    let server_addr = server.node_addr().await;

    client.add_node_addr(server_addr.clone())?;

    let server_for_task = server.clone();
    let server_task = tokio::spawn(async move {
        let connection = server_for_task
            .accept()
            .await?
            .expect("server accepts connection");
        let (mut send, mut recv) = connection.accept_bi().await?;

        let bytes = recv.read_to_end(MAX_FRAME_BYTES + 4).await?;
        let mut buf = BytesMut::from(bytes.as_slice());
        let client_hello = decode_message_frame::<Hello>(&mut buf)?.expect("complete Hello frame");
        assert!(buf.is_empty());

        let server_hello = Hello {
            min_minor: 2,
            max_minor: EXPECTED_MINOR,
            capabilities: vec!["shared".to_string(), "server-only".to_string()],
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)?,
            catalog_generation: 7,
        };
        let hello_ok = negotiate(&client_hello, &server_hello)?;

        send.write_all(&encode_frame(&hello_ok)).await?;
        send.finish()?;
        connection.closed().await;

        TestResult::Ok(hello_ok.protocol_minor)
    });

    let connection = client.connect(server_addr).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    let client_hello = Hello {
        min_minor: 1,
        max_minor: 5,
        capabilities: vec!["shared".to_string(), "client-only".to_string()],
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)?,
        catalog_generation: 0,
    };

    send.write_all(&encode_frame(&client_hello)).await?;
    send.finish()?;

    let bytes = recv.read_to_end(MAX_FRAME_BYTES + 4).await?;
    let mut buf = BytesMut::from(bytes.as_slice());
    let hello_ok = decode_message_frame::<HelloOk>(&mut buf)?.expect("complete HelloOk frame");
    assert!(buf.is_empty());
    assert_eq!(hello_ok.protocol_minor, EXPECTED_MINOR);

    connection.close(0u8.into(), b"done");

    let server_minor = server_task.await??;
    assert_eq!(server_minor, EXPECTED_MINOR);

    client.close().await;
    server.close().await;

    TestResult::Ok(())
}
