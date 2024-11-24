use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Error, Json, Router,
};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};

// Set DB_NAME here
const DB_NAME: &'static str = "temp";

async fn migrate() {
    let mut conn = rusqlite::Connection::open(format!("./{DB_NAME}.sqlite3")).unwrap();

    // 1️⃣ Define migrations
    let migrations = Migrations::new(vec![M::up(
        "CREATE TABLE users(id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE);",
    )]);

    // Apply some PRAGMA, often better to do it outside of migrations
    conn.pragma_update_and_check(None, "journal_mode", &"WAL", |_| Ok(()))
        .unwrap();

    // 2️⃣ Update the database schema, atomically
    migrations.to_latest(&mut conn).unwrap();
}

#[tokio::main]
async fn main() {
    let conn = tokio_rusqlite::Connection::open("./temp.sqlite3")
        .await
        .unwrap();

    // Run any new migrations
    migrate().await;

    // initialize tracing
    tracing_subscriber::fmt::init();

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        // `POST /users` goes to `create_user`
        .route("/users", post(create_user))
        .route("/users", get(get_users))
        .with_state(conn);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
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
