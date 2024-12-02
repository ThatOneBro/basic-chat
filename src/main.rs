use axum::{
    extract::State,
    http::StatusCode,
    routing::{any, get, post},
    Error, Json, Router,
};
use dotenv::dotenv;
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::env;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod websocket;

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
        .route("/ws", any(websocket::ws_handler))
        .with_state(conn)
        .layer(CorsLayer::permissive());

    let port = env::var("PORT").unwrap_or("3000".to_string());
    println!("Attempting to bind to port {port}...");

    // run our app with hyper, listening globally on `port`
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();
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
    State(conn): State<tokio_rusqlite::Connection>,
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
    conn.call_unwrap(|conn| {
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

async fn get_users(
    State(conn): State<tokio_rusqlite::Connection>,
) -> (StatusCode, Json<Vec<User>>) {
    let users = conn
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
    State(conn): State<tokio_rusqlite::Connection>,
    Json(payload): Json<CreateMessage>,
) -> (StatusCode, Json<Message>) {
    let msg: Message = Message {
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
    conn.call_unwrap(move |conn| match msg_copy.reply_to {
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

async fn get_messages(
    State(conn): State<tokio_rusqlite::Connection>,
) -> (StatusCode, Json<Vec<Message>>) {
    let messages = conn
        .call_unwrap(|conn| -> Result<Vec<Message>, Error> {
            let mut stmt = conn
                .prepare("SELECT * FROM messages ORDER BY time DESC LIMIT 100;")
                .unwrap();
            let messages = stmt
                .query_map([], |row| {
                    Ok(Message {
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
                .collect::<std::result::Result<Vec<Message>, rusqlite::Error>>()
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

#[derive(Deserialize)]
struct CreateMessage {
    time: u64,
    // TODO: Remove user_id and username, or potentially just validate them against values in JWT later (to extra processing)
    user_id: String,
    username: String,
    text: String,
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    reply_to: Option<String>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta: Option<EncryptMeta>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta_sig: Option<String>,
}

#[derive(Serialize, Clone)]
struct Message {
    id: String,
    time: u64,
    user_id: String,
    username: String,
    text: String,
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    reply_to: Option<String>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta: Option<EncryptMeta>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta_sig: Option<String>,
}
