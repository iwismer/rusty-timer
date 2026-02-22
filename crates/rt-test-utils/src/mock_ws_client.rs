use futures_util::{SinkExt, StreamExt};
use rt_protocol::WsMessage;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::Message;

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub struct MockWsClient {
    write: futures_util::stream::SplitSink<WsStream, Message>,
    read: futures_util::stream::SplitStream<WsStream>,
}

impl MockWsClient {
    pub async fn connect(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let (ws_stream, _response) = tokio_tungstenite::connect_async(url).await?;
        let (write, read) = ws_stream.split();
        Ok(Self { write, read })
    }

    pub async fn connect_with_token(
        url: &str,
        token: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;
        let uri: tokio_tungstenite::tungstenite::http::Uri = url.parse()?;
        let host = uri.host().unwrap_or("localhost").to_owned();
        let port = uri.port_u16();
        let host_header = if let Some(p) = port {
            format!("{}:{}", host, p)
        } else {
            host
        };
        let request = Request::builder()
            .uri(url)
            .header("Host", host_header)
            .header("Authorization", format!("Bearer {}", token))
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .body(())?;
        let (ws_stream, _response) = tokio_tungstenite::connect_async(request).await?;
        let (write, read) = ws_stream.split();
        Ok(Self { write, read })
    }

    pub async fn send_message(
        &mut self,
        msg: &WsMessage,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string(msg)?;
        self.write.send(Message::Text(json.into())).await?;
        Ok(())
    }

    pub async fn recv_message(&mut self) -> Result<WsMessage, Box<dyn std::error::Error>> {
        loop {
            match self.read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let msg: WsMessage = serde_json::from_str(&text)?;
                    return Ok(msg);
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) => return Err("connection closed by server".into()),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => return Err("connection stream ended".into()),
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.write.send(Message::Close(None)).await?;
        Ok(())
    }
}
