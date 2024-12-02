use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{any, get, post},
    Error, Json, Router,
};
use axum_extra::{headers, TypedHeader};
use dotenv::dotenv;
use futures::{SinkExt, StreamExt};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

//allows to extract the IP of connecting user
use axum::extract::connect_info::ConnectInfo;

mod msg;

async fn migrate(db_path: &String) {
    let mut conn = rusqlite::Connection::open(db_path).unwrap();

    // 1️⃣ Define migrations
    let migrations = Migrations::new(vec![
        M::up("CREATE TABLE users(id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE);"),
        M::up("CREATE TABLE messages(id TEXT PRIMARY KEY, time INTEGER NOT NULL, user_id TEXT NOT NULL, username TEXT NOT NULL, text TEXT NOT NULL, reply_to TEXT);"),
        M::up("ALTER TABLE messages ADD COLUMN channel TEXT NOT NULL DEFAULT 'main';"),
    ]);

    // Apply some PRAGMA, often better to do it outside of migrations
    conn.pragma_update_and_check(None, "journal_mode", &"WAL", |_| Ok(()))
        .unwrap();

    // 2️⃣ Update the database schema, atomically
    migrations.to_latest(&mut conn).unwrap();
}

#[tokio::main]
async fn main() {
    // Load from .env file
    dotenv().ok();

    let db_path = std::env::var("SQLITE_DB_PATH").expect("SQLITE_DB_PATH must be set in env.");

    // Run any new migrations
    migrate(&db_path).await;

    // Set up db connection
    let conn = tokio_rusqlite::Connection::open(db_path).await.unwrap();

    // initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
                format!("tower_http=debug").into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        // `POST /users` goes to `create_user`
        .route("/users", post(create_user))
        .route("/users", get(get_users))
        .route("/messages", post(create_message))
        .route("/messages", get(get_messages))
        .route("/ws", any(ws_handler))
        .with_state(Arc::new(AppState::new(conn)))
        .layer(CorsLayer::permissive());

    let port = env::var("PORT")
        .unwrap_or("3000".into())
        .parse::<u16>()
        .unwrap();
    println!("Attempting to bind to port {port}...");

    // run our app with hyper, listening globally on `port`
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

// basic handler that responds with a static string
async fn root() -> &'static str {
    "Hello, World!"
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    // this argument tells axum to parse the request body
    // as JSON into a `CreateUser` type
    Json(payload): Json<CreateUser>,
) -> (StatusCode, Json<User>) {
    // insert your application logic here
    let user: User = User {
        id: uuidv7::create(),
        username: payload.username,
    };

    let user_copy = user.clone();

    // Add user to users table
    state
        .conn
        .call_unwrap(|conn| {
            conn.execute(
                "INSERT INTO users VALUES (?, ?)",
                [user_copy.id, user_copy.username],
            )
            .unwrap();
        })
        .await;

    // this will be converted into a JSON response
    // with a status code of `201 Created`
    (StatusCode::CREATED, Json(user))
}

async fn get_users(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Vec<User>>) {
    let users = state
        .conn
        .call_unwrap(|conn| -> Result<Vec<User>, Error> {
            let mut stmt = conn
                .prepare("SELECT id, username FROM users LIMIT 100;")
                .unwrap();
            let users = stmt
                .query_map([], |row| {
                    Ok(User {
                        id: row.get(0)?,
                        username: row.get(1)?,
                    })
                })
                .unwrap()
                .collect::<std::result::Result<Vec<User>, rusqlite::Error>>()
                .unwrap();

            Ok(users)
        })
        .await
        .unwrap();

    (StatusCode::OK, Json(users))
}

async fn create_message(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<msg::CreateMessage>,
) -> (StatusCode, Json<msg::Message>) {
    let msg: msg::Message = msg::Message {
        id: uuidv7::create(),
        time: payload.time,
        user_id: payload.user_id,
        username: payload.username,
        text: payload.text,
        channel: payload.channel,
        reply_to: payload.reply_to,
    };

    let msg_copy = msg.clone();

    // Add user to users table
    state.conn.call_unwrap(move |conn| match msg_copy.reply_to {
        Some(reply_to) => {
            conn.execute(
                "INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?)",
                [
                    msg_copy.id,
                    msg_copy.time.to_string(),
                    msg_copy.user_id,
                    msg_copy.username,
                    msg_copy.text,
                    reply_to,
                    msg_copy.channel,
                ],
            )
            .unwrap();
        }
        None => {
            conn.execute(
                "INSERT INTO messages (id, time, user_id, username, text, channel) VALUES (?, ?, ?, ?, ?, ?)",
                [
                    msg_copy.id,
                    msg_copy.time.to_string(),
                    msg_copy.user_id,
                    msg_copy.username,
                    msg_copy.text,
                    msg_copy.channel,
                ],
            )
            .unwrap();
        }
    })
    .await;

    // this will be converted into a JSON response
    // with a status code of `201 Created`
    (StatusCode::CREATED, Json(msg))
}

async fn get_messages(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Vec<msg::Message>>) {
    let messages = state
        .conn
        .call_unwrap(|conn| -> Result<Vec<msg::Message>, Error> {
            let mut stmt = conn
                .prepare("SELECT * FROM messages ORDER BY time DESC LIMIT 100;")
                .unwrap();
            let messages = stmt
                .query_map([], |row| {
                    Ok(msg::Message {
                        id: row.get(0)?,
                        time: row.get(1)?,
                        user_id: row.get(2)?,
                        username: row.get(3)?,
                        text: row.get(4)?,
                        channel: row.get(6)?,
                        reply_to: row.get(5).unwrap_or(None),
                        // encrypt_meta: row.get(6).unwrap_or(None),
                        // encrypt_meta_sig: row.get(7).unwrap_or(None),
                    })
                })
                .unwrap()
                .collect::<std::result::Result<Vec<msg::Message>, rusqlite::Error>>()
                .unwrap();

            Ok(messages)
        })
        .await
        .unwrap();

    (StatusCode::OK, Json(messages))
}

#[derive(Serialize, Deserialize, Clone)]
enum EncryptAlg {
    X25519,
}

// the input to our `create_user` handler
#[derive(Deserialize)]
struct CreateUser {
    username: String,
}

// the output to our `create_user` handler
#[derive(Serialize, Clone)]
struct User {
    id: String,
    username: String,
}

// #[derive(Serialize, Deserialize, Clone)]
// struct EncryptMeta {
//     time: u64,
//     alg: EncryptAlg,
//     user_id: String,
//     public_key: String,
// }

// Reference: https://gist.github.com/hexcowboy/8ebcf13a5d3b681aa6c684ad51dd6e0c
async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    println!("{user_agent} at {addr} connected.");
    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    ws.on_upgrade(move |socket| handle_upgrade(socket, addr, state))
}

async fn handle_upgrade(socket: WebSocket, _addr: SocketAddr, state: Arc<AppState>) {
    // split the websocket stream into a sender (sink) and receiver (stream)
    let (mut sink, mut stream) = socket.split();
    // create an mpsc so we can send messages to the sink from multiple threads
    let (sender, mut receiver) = mpsc::channel::<String>(16);

    // spawn a task that forwards messages from the mpsc to the sink
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if sink.send(message.into()).await.is_err() {
                break;
            }
        }
    });

    // subscribe to the chat channel
    let mut rx_chat = state.tx.subscribe();

    // whenever a chat is sent to rx_chat, forward it to the mpsc
    let send_task_sender = sender.clone();
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx_chat.recv().await {
            if send_task_sender
                .send(format!("New message: {}", msg))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // clone the tx channel so we can send messages to it
    let tx_chat = state.tx.clone();

    // whenever a user sends a chat, send it to the tx_chat
    let recv_task_sender = sender.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = stream.next().await {
            let _ = tx_chat.send(format!("{}", text));
            if recv_task_sender
                .send(String::from("Your message has been sent"))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}

struct AppState {
    // channel used to send messages to all connected clients
    tx: broadcast::Sender<String>,
    conn: tokio_rusqlite::Connection,
}

impl AppState {
    fn new(conn: tokio_rusqlite::Connection) -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { tx, conn }
    }
}
